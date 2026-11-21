//! Image croppers: a box with ratio presets, handles, a grid, zoom, rotate
//! and flip, cropped to a new image, and a round frame cropped to an avatar.

use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, BoxShadow, Context, Entity, Image, ImageFormat, ImageSource, IntoElement,
    ObjectFit, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, StyledImage,
    Window, WindowBounds, WindowOptions, div, hsla, img, point, px, relative, rgb, size,
};
use ui_sdk_button::{Button, ButtonOptions};
use ui_sdk_image::{CropImageOptions, CropShape, crop_image};
use ui_sdk_image_cropper::{
    ImageCrop, ImageCropper, ImageCropperFlipDirection, ImageCropperFlipProps, ImageCropperHandlePosition,
    ImageCropperHandleProps, ImageCropperMode, ImageCropperOptions, ImageCropperShape,
};
use ui_sdk_kernel::ActionOutcome;

const PHOTO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="640" height="420"><rect width="640" height="420" fill="#e5e5e5"/><circle cx="460" cy="150" r="56" fill="#fafafa"/><path d="M0 280 120 170l110 100 120-130 130 125 90-60 70 40v175H0z" fill="#a3a3a3"/><path d="M0 330l160-80 120 70 150-80 210 85v95H0z" fill="#525252"/><path d="M0 380q160-35 320-10t320-5v55H0z" fill="#262626"/></svg>"##;

const RATIOS: [(&str, Option<f64>); 4] = [
    ("Free", None),
    ("Square", Some(1.0)),
    ("4:3", Some(4.0 / 3.0)),
    ("16:9", Some(16.0 / 9.0)),
];

struct Example {
    photo: ImageSource,
    cropper: Entity<ImageCropper>,
    ratio: Option<f64>,
    presets: Vec<Entity<Button>>,
    crop: Entity<Button>,
    result: Option<Arc<Image>>,
    avatar: Entity<ImageCropper>,
    save: Entity<Button>,
    avatar_result: Option<Arc<Image>>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let photo = ImageSource::from(Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            PHOTO.as_bytes().to_vec(),
        )));
        let options = photo_options(&photo, None);
        let cropper = cx.new(|cx| ImageCropper::new(options, window, cx));
        let this = cx.entity().downgrade();
        let presets = RATIOS
            .iter()
            .map(|(_, ratio)| {
                let (this, ratio) = (this.clone(), *ratio);
                cx.new(|cx| {
                    Button::new(
                        ButtonOptions::default().action(move |_event, _details, _window, cx| {
                            this.update(cx, |example, cx| {
                                example.ratio = ratio;
                                cx.notify();
                            })
                            .ok();
                            ActionOutcome::Done
                        }),
                        window,
                        cx,
                    )
                })
            })
            .collect();
        let crops = this.clone();
        let crop = cx.new(|cx| {
            Button::new(
                ButtonOptions::default().action(move |_event, _details, _window, cx| {
                    crops.update(cx, |example, cx| example.crop_photo(cx)).ok();
                    ActionOutcome::Done
                }),
                window,
                cx,
            )
        });
        let avatar = cx.new(|cx| {
            ImageCropper::new(
                ImageCropperOptions::default()
                    .src(photo.clone())
                    .mode(ImageCropperMode::Frame)
                    .shape(ImageCropperShape::Circle)
                    .aspect_ratio(1.0)
                    .on_crop_commit(|crop, details, _window, _cx| {
                        log::info!("avatar {crop:?} ({})", details.reason.as_str());
                    }),
                window,
                cx,
            )
        });
        let saves = this;
        let save = cx.new(|cx| {
            Button::new(
                ButtonOptions::default().action(move |_event, _details, _window, cx| {
                    saves.update(cx, |example, cx| example.save_avatar(cx)).ok();
                    ActionOutcome::Done
                }),
                window,
                cx,
            )
        });
        Self {
            photo,
            cropper,
            ratio: None,
            presets,
            crop,
            result: None,
            avatar,
            save,
            avatar_result: None,
        }
    }

    // Crops the photo to its crop, as a new image.
    fn crop_photo(&mut self, cx: &mut Context<Self>) {
        let Some(crop) = self.cropper.read(cx).get_crop() else {
            return;
        };
        self.cut(crop, CropImageOptions::default(), false, cx);
    }

    // Crops the avatar to a round image 128 pixels across.
    fn save_avatar(&mut self, cx: &mut Context<Self>) {
        let Some(crop) = self.avatar.read(cx).get_crop() else {
            return;
        };
        let options = CropImageOptions::default()
            .width(128.0)
            .height(128.0)
            .shape(CropShape::Circle);
        self.cut(crop, options, true, cx);
    }

    fn cut(&mut self, crop: ImageCrop, options: CropImageOptions, avatar: bool, cx: &mut Context<Self>) {
        let task = crop_image(self.photo.clone(), crop, options, cx);
        cx.spawn(async move |this, cx| {
            let Ok(file) = task.await else {
                return;
            };
            this.update(cx, |example, cx| {
                if avatar {
                    example.avatar_result = Some(file.image);
                } else {
                    example.result = Some(file.image);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn photo_options(photo: &ImageSource, ratio: Option<f64>) -> ImageCropperOptions {
    ImageCropperOptions::default()
        .src(photo.clone())
        .aspect_ratio(ratio)
        .min_width(64.0)
        .min_height(64.0)
        .on_crop_commit(|crop, details, _window, _cx| {
            log::info!("photo {crop:?} ({})", details.reason.as_str());
        })
}

fn button(button: &Entity<Button>, text: &'static str, chosen: bool, cx: &App) -> impl IntoElement {
    Button::root(button, cx)
        .aria_label(SharedString::from(text))
        .px_3()
        .py_1()
        .rounded_md()
        .bg(if chosen { rgb(0x1f2937) } else { rgb(0xf3f4f6) })
        .text_color(if chosen { rgb(0xffffff) } else { rgb(0x111827) })
        .child(text)
}

// The grid's lines across the selection, in thirds, while a drag is on.
fn thirds() -> Vec<gpui::Div> {
    let line = hsla(0., 0., 1., 0.6);
    [1. / 3., 2. / 3.]
        .into_iter()
        .flat_map(|at| {
            [
                div()
                    .absolute()
                    .top(relative(at))
                    .left_0()
                    .right_0()
                    .h(px(1.))
                    .bg(line),
                div()
                    .absolute()
                    .left(relative(at))
                    .top_0()
                    .bottom_0()
                    .w(px(1.))
                    .bg(line),
            ]
        })
        .collect()
}

// A viewport with the image, the selection and its grid, and the handles.
fn viewport(entity: &Entity<ImageCropper>, width: f32, handles: bool, cx: &App) -> impl IntoElement {
    let dim = BoxShadow {
        color: hsla(0., 0., 0., 0.5),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(2000.),
        inset: false,
    };
    let selection = ImageCropper::selection(entity, cx)
        .border_1()
        .border_color(rgb(0xffffff))
        .shadow(vec![dim])
        .with(|el, state| el.when(state.shape == ImageCropperShape::Circle, |el| el.rounded_full()))
        .child(
            ImageCropper::grid(entity, cx)
                .with(|el, state| el.when(state.dragging, |el| el.children(thirds()))),
        );
    let handles = handles.then(|| {
        ImageCropperHandlePosition::ALL.map(|position| {
            ImageCropper::handle(entity, ImageCropperHandleProps::new(position), cx)
                .size(px(10.))
                .ml(px(-5.))
                .mt(px(-5.))
                .rounded_sm()
                .bg(rgb(0xffffff))
        })
    });
    ImageCropper::viewport(entity, cx)
        .w(px(width))
        .h(px(width * 2. / 3.))
        .p_3()
        .rounded_md()
        .bg(rgb(0x111827))
        .child(ImageCropper::image(entity, cx).aria_label("Mountains at sunset"))
        .child(selection)
        .children(handles.into_iter().flatten())
}

fn zoom(entity: &Entity<ImageCropper>, cx: &App) -> impl IntoElement {
    ImageCropper::zoom(entity, cx)
        .w(px(160.))
        .h(px(20.))
        .flex()
        .items_center()
        .with(|el, state| {
            el.child(
                div()
                    .relative()
                    .w_full()
                    .h(px(4.))
                    .rounded_full()
                    .bg(rgb(0xd1d5db))
                    .child(
                        div()
                            .absolute()
                            .top(px(-5.))
                            .left(relative(state.fraction as f32))
                            .ml(px(-7.))
                            .size(px(14.))
                            .rounded_full()
                            .bg(if state.disabled {
                                rgb(0x9ca3af)
                            } else {
                                rgb(0x1f2937)
                            }),
                    ),
            )
        })
}

fn result(image: Option<&Arc<Image>>, round: bool) -> impl IntoElement {
    div().children(image.map(|image| {
        img(ImageSource::from(image.clone()))
            .h(px(96.))
            .object_fit(ObjectFit::Contain)
            .when(round, |el| el.rounded_full())
    }))
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let options = photo_options(&self.photo, self.ratio);
        self.cropper
            .update(cx, |cropper, cx| cropper.set_options(options, window, cx));
        let cx: &App = cx;
        let presets = RATIOS
            .iter()
            .zip(&self.presets)
            .map(|((label, ratio), entity)| button(entity, label, *ratio == self.ratio, cx));
        let photo = ImageCropper::root(&self.cropper, cx)
            .aria_label("Photo")
            .flex()
            .flex_col()
            .items_start()
            .gap_2()
            .child(div().flex().gap_2().children(presets))
            .child(viewport(&self.cropper, 420., true, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(zoom(&self.cropper, cx))
                    .child(
                        ImageCropper::rotate_button(&self.cropper, cx)
                            .aria_label("Rotate")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0xf3f4f6))
                            .child("Rotate"),
                    )
                    .child(
                        ImageCropper::flip_button(&self.cropper, ImageCropperFlipProps::default(), cx)
                            .aria_label("Flip")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0xf3f4f6))
                            .child("Flip"),
                    )
                    .child(
                        ImageCropper::flip_button(
                            &self.cropper,
                            ImageCropperFlipProps::default().direction(ImageCropperFlipDirection::Vertical),
                            cx,
                        )
                        .aria_label("Flip vertically")
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(0xf3f4f6))
                        .child("Flip vertically"),
                    )
                    .child(button(&self.crop, "Crop", false, cx)),
            )
            .child(result(self.result.as_ref(), false));
        let avatar = ImageCropper::root(&self.avatar, cx)
            .aria_label("Avatar")
            .flex()
            .flex_col()
            .items_start()
            .gap_2()
            .child(viewport(&self.avatar, 240., false, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(zoom(&self.avatar, cx))
                    .child(button(&self.save, "Save avatar", false, cx)),
            )
            .child(result(self.avatar_result.as_ref(), true));
        div()
            .size_full()
            .flex()
            .items_start()
            .gap_8()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(photo)
            .child(avatar)
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(820.), px(560.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Example::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
