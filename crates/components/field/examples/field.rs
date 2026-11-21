//! Fields in a form: a text field of the app's own through the field's
//! control part, a switch with its error, and a slider's field and a field
//! set a switch disables.

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, Context, Entity, Focusable, IntoElement, ParentElement, Render,
    Rgba, StatefulInteractiveElement, Styled, Subscription, Window, WindowBounds, WindowOptions,
    div, px, rgb, size,
};
use ui_sdk_button::{Button, ButtonOptions, ButtonState, ButtonType};
use ui_sdk_field::{
    Field, FieldErrorProps, FieldGroup, FieldGroupOptions, FieldOptions, FieldSet,
    FieldSetOptions, FieldState, ValidityKey,
};
use ui_sdk_forms::{
    Form, FormOptions, Label, LabelOptions, ValueInputOptions, bind_value_input,
    submit_implicitly,
};
use ui_sdk_kernel::Part;
use ui_sdk_native::{TextField, TextFieldOptions, TextFieldState};
use ui_sdk_platform::live_regions;
use ui_sdk_slider::{Slider, SliderOptions};
use ui_sdk_switch::{Switch, SwitchOptions, SwitchState};

const MAX_LENGTH: usize = 20;

struct Example {
    form: Form,
    username: Entity<Field>,
    username_input: Entity<TextField>,
    two_factor: Entity<Field>,
    two_factor_switch: Entity<Switch>,
    notifications: Entity<FieldSet>,
    group: Entity<FieldGroup>,
    mentions: Entity<Field>,
    mentions_switch: Entity<Switch>,
    follows: Entity<Field>,
    follows_switch: Entity<Switch>,
    mute: Entity<Switch>,
    mute_label: Label,
    muted: bool,
    volume: Entity<Field>,
    volume_slider: Entity<Slider>,
    save: Entity<Button>,
    reset: Entity<Button>,
    _reset: Subscription,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let form = Form::new(FormOptions::default().on_submit(|data, _details, _window, _cx| {
            log::info!("submitted {}", data.to_url_encoded());
        }));
        let mut field = |options: FieldOptions, cx: &mut Context<Self>| {
            cx.new(|cx| Field::new(options, window, cx))
        };
        let username = field(FieldOptions::default().name("username"), cx);
        let two_factor = field(FieldOptions::default(), cx);
        let mentions = field(FieldOptions::default(), cx);
        let follows = field(FieldOptions::default(), cx);
        let volume = field(FieldOptions::default().name("volume"), cx);
        let submitting = form.clone();
        let username_input = cx.new(|cx| {
            TextField::new(
                TextFieldOptions::default()
                    .placeholder("hollis")
                    .max_length(MAX_LENGTH)
                    .on_submit(move |_input, window, cx| {
                        submit_implicitly(Some(&submitting), window, cx)
                    }),
                window,
                cx,
            )
        });
        let this = cx.entity().downgrade();
        let mut switch = |options: SwitchOptions, cx: &mut Context<Self>| {
            cx.new(|cx| Switch::new(options, window, cx))
        };
        let two_factor_switch = switch(
            SwitchOptions::default().name("two-factor").required(true),
            cx,
        );
        let mentions_switch = switch(
            SwitchOptions::default().name("mentions").default_checked(true),
            cx,
        );
        let follows_switch = switch(SwitchOptions::default().name("follows"), cx);
        let mute = switch(
            SwitchOptions::default().on_checked_change(move |checked, _details, _window, cx| {
                let checked = *checked;
                this.update(cx, |example, cx| {
                    example.muted = checked;
                    cx.notify();
                })
                .ok();
            }),
            cx,
        );
        // A form reset empties the text field, as it would a native input.
        let input = username_input.clone();
        let _reset = username.read(cx).control_internals().on_reset(cx, move |_example, window, cx| {
            input.update(cx, |input, cx| input.reset(window, cx));
        });
        let volume_slider = cx.new(|cx| Slider::new(SliderOptions::default().default_value(50.0), window, cx));
        let notifications = cx.new(|cx| FieldSet::new(FieldSetOptions::default(), window, cx));
        let group = cx.new(|cx| FieldGroup::new(FieldGroupOptions::default(), window, cx));
        let save = cx.new(|cx| Button::new(ButtonOptions::default().r#type(ButtonType::Submit), window, cx));
        let reset = cx.new(|cx| Button::new(ButtonOptions::default().r#type(ButtonType::Reset), window, cx));
        Self {
            form,
            username,
            username_input,
            two_factor,
            two_factor_switch,
            notifications,
            group,
            mentions,
            mentions_switch,
            follows,
            follows_switch,
            mute,
            mute_label: Label::new(LabelOptions::default().text("Mute")),
            muted: false,
            volume,
            volume_slider,
            save,
            reset,
            _reset,
        }
    }
}

const RED: u32 = 0xdc2626;

fn border(state: &TextFieldState, field: &FieldState) -> Rgba {
    if field.invalid {
        rgb(RED)
    } else if state.focused {
        rgb(0x2563eb)
    } else {
        rgb(0xd1d5db)
    }
}

fn track(state: &SwitchState) -> Rgba {
    match (state.checked, state.disabled) {
        (_, true) => rgb(0xd1d5db),
        (true, false) => rgb(0x2563eb),
        (false, false) => rgb(0x9ca3af),
    }
}

fn switch(switch: &Entity<Switch>, cx: &App) -> Part<SwitchState> {
    let thumb = Switch::thumb(switch, cx)
        .size(px(16.))
        .rounded_full()
        .bg(rgb(0xffffff));
    Switch::root(switch, cx)
        .with(|el, state| el.bg(track(state)).when(state.checked, |el| el.justify_end()))
        .flex()
        .w(px(36.))
        .p(px(2.))
        .rounded_full()
        .child(thumb)
}

fn label(field: &Entity<Field>, text: &'static str, cx: &App) -> Part<FieldState> {
    Field::label(field, cx)
        .aria_label(text)
        .with(|el, state| {
            el.when(state.invalid, |el| el.text_color(rgb(RED)))
                .when(state.disabled, |el| el.text_color(rgb(0x9ca3af)))
        })
        .child(text)
}

fn switch_field(field: &Entity<Field>, control: &Entity<Switch>, text: &'static str, cx: &App) -> impl IntoElement {
    Field::root(field, cx)
        .flex()
        .items_center()
        .gap_2()
        .child(switch(control, cx))
        .child(label(field, text, cx))
}

fn button(button: &Entity<Button>, text: &'static str, cx: &App) -> impl IntoElement {
    Button::root(button, cx)
        .aria_label(text)
        .with(|el, state: &ButtonState| el.when(state.pending, |el| el.opacity(0.6)))
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(0xf3f4f6))
        .child(text)
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = self.muted;
        self.notifications.update(cx, |set, cx| {
            set.set_options(FieldSetOptions::default().disabled(muted), cx)
        });
        self.volume.update(cx, |field, cx| {
            field.set_options(FieldOptions::default().name("volume").disabled(muted), cx)
        });
        // The text field is the app's own, so it reports its value as a
        // native input would.
        let value = self.username_input.read(cx).value().clone();
        let count = format!("{} / {MAX_LENGTH}", value.chars().count());
        bind_value_input(
            self.username.read(cx).control_internals(),
            ValueInputOptions {
                value,
                required: true,
                ..Default::default()
            },
        );
        let focus = self.username_input.focus_handle(cx);
        let field = self.username.read(cx).state();
        let input = TextField::root(&self.username_input, cx)
            .with(move |el, state| el.border_color(border(state, &field)))
            .w(px(240.))
            .border_1()
            .rounded_md()
            .px_2()
            .py_1();
        let username = Field::root(&self.username, cx)
            .flex()
            .flex_col()
            .gap_1()
            .child(label(&self.username, "Username", cx))
            .child(Field::control(&self.username, input, Some(&focus), cx))
            .child(
                Field::description(&self.username, "count", cx)
                    .aria_label(count.clone())
                    .text_sm()
                    .text_color(rgb(0x6b7280))
                    .child(count),
            )
            .children(
                Field::error(&self.username, "problem", FieldErrorProps::new(), cx).map(|error| {
                    error
                        .with(|el, state| el.child(state.message.clone()))
                        .text_sm()
                        .text_color(rgb(RED))
                }),
            );
        let two_factor = Field::root(&self.two_factor, cx)
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(switch(&self.two_factor_switch, cx))
                    .child(label(&self.two_factor, "Two-factor", cx)),
            )
            .children(
                Field::error(
                    &self.two_factor,
                    "missing",
                    FieldErrorProps::new()
                        .r#match(ValidityKey::ValueMissing)
                        .text("Required"),
                    cx,
                )
                .map(|error| error.text_sm().text_color(rgb(RED)).child("Required")),
            );
        let mute = self
            .mute_label
            .root()
            .flex()
            .items_center()
            .gap_2()
            .child(switch(&self.mute, cx).aria_label("Mute"))
            .child("Mute");
        // The field's own `disabled` disables the slider inside it.
        let slider = &self.volume_slider;
        let ink = |disabled: bool| if disabled { rgb(0x9ca3af) } else { rgb(0x1f2937) };
        let thumb = Slider::thumb(slider, 0, cx)
            .size_4()
            .rounded_full()
            .border_2()
            .bg(rgb(0xffffff))
            .with(move |el, state| el.border_color(ink(state.disabled)));
        let track = Slider::track(slider, cx)
            .w_full()
            .h(px(4.))
            .rounded_full()
            .bg(rgb(0xe5e7eb))
            .child(
                Slider::indicator(slider, cx)
                    .rounded_full()
                    .with(move |el, state| el.bg(ink(state.disabled))),
            )
            .child(thumb);
        let volume = Field::root(&self.volume, cx)
            .flex()
            .flex_col()
            .gap_1()
            .child(label(&self.volume, "Volume", cx))
            .child(
                Slider::root(slider, cx).child(
                    Slider::control(slider, cx)
                        .flex()
                        .items_center()
                        .w(px(200.))
                        .h(px(20.))
                        .child(track),
                ),
            );
        let notifications = FieldSet::set(&self.notifications, cx)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                FieldSet::legend(&self.notifications, cx)
                    .aria_label("Notifications")
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Notifications"),
            )
            .child(
                FieldGroup::group(&self.group, cx)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(switch_field(&self.mentions, &self.mentions_switch, "Mentions", cx))
                    .child(switch_field(&self.follows, &self.follows_switch, "Follows", cx)),
            );
        div()
            .size_full()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(live_regions(window))
            .child(
                self.form
                    .root()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(username)
                    .child(two_factor)
                    .child(mute)
                    .child(volume)
                    .child(notifications)
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(button(&self.save, "Save", cx))
                            .child(button(&self.reset, "Reset", cx)),
                    ),
            )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(360.), px(480.)), cx);
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
