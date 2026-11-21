//! Radio groups: one with a default, one controlled, one with a disabled
//! radio, and one read-only.

use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, ease_in_out, px,
    rgb, size,
};
use ui_sdk_forms::{Label, LabelOptions};
use ui_sdk_motion::{PresenceTiming, Timing};
use ui_sdk_radio::{RadioGroup, RadioGroupOptions, RadioIndicatorProps, RadioProps};

struct Choice {
    value: &'static str,
    text: &'static str,
    disabled: bool,
    label: Label,
}

fn choices(list: &[(&'static str, &'static str)], disabled: &[&str]) -> Vec<Choice> {
    list.iter()
        .map(|&(value, text)| Choice {
            value,
            text,
            disabled: disabled.contains(&value),
            label: Label::new(LabelOptions::default().text(text)),
        })
        .collect()
}

struct Group {
    entity: Entity<RadioGroup>,
    title: &'static str,
    choices: Vec<Choice>,
}

struct Example {
    shipping: Group,
    plan: Group,
    backups: Group,
    theme: Group,
    plan_value: Option<SharedString>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let timing = PresenceTiming::new(Timing::ms(150).easing(ease_in_out));
        let shipping = Group {
            entity: cx.new(|cx| {
                RadioGroup::new(
                    RadioGroupOptions::default()
                        .name("shipping")
                        .default_value(Some("standard"))
                        .indicator_timing(timing.clone()),
                    window,
                    cx,
                )
            }),
            title: "Shipping",
            choices: choices(
                &[("standard", "Standard"), ("express", "Express"), ("overnight", "Overnight")],
                &[],
            ),
        };
        let this = cx.entity().downgrade();
        let plan = Group {
            entity: cx.new(|cx| {
                RadioGroup::new(
                    RadioGroupOptions::default()
                        .value(Some("pro"))
                        .indicator_timing(timing.clone())
                        .on_value_change(move |value, _details, _window, cx| {
                            let value = value.clone();
                            this.update(cx, |example, cx| {
                                example.plan_value = value;
                                cx.notify();
                            })
                            .ok();
                        }),
                    window,
                    cx,
                )
            }),
            title: "Plan",
            choices: choices(&[("free", "Free"), ("pro", "Pro"), ("team", "Team")], &[]),
        };
        let backups = Group {
            entity: cx.new(|cx| {
                RadioGroup::new(
                    RadioGroupOptions::default()
                        .default_value(Some("daily"))
                        .r#loop(false),
                    window,
                    cx,
                )
            }),
            title: "Backups",
            choices: choices(
                &[("daily", "Daily"), ("hourly", "Hourly"), ("weekly", "Weekly")],
                &["hourly"],
            ),
        };
        let theme = Group {
            entity: cx.new(|cx| {
                RadioGroup::new(
                    RadioGroupOptions::default()
                        .default_value(Some("system"))
                        .read_only(true),
                    window,
                    cx,
                )
            }),
            title: "Theme",
            choices: choices(&[("light", "Light"), ("dark", "Dark"), ("system", "System")], &[]),
        };
        Self {
            shipping,
            plan,
            backups,
            theme,
            plan_value: Some("pro".into()),
        }
    }
}

fn radio(group: &Entity<RadioGroup>, choice: &Choice, cx: &App) -> impl IntoElement {
    let indicator = RadioGroup::indicator(group, choice.value, RadioIndicatorProps::default(), cx)
        .map(|indicator| {
            indicator
                .size_2()
                .rounded_full()
                .bg(rgb(0x1f2937))
                .with(|el, state| el.opacity(state.presence.progress))
        });
    let button = RadioGroup::root(group, choice.value, RadioProps::default().disabled(choice.disabled), cx)
        .size_4()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(rgb(0x6b7280))
        .with(|el, state| el.opacity(if state.disabled { 0.5 } else { 1.0 }))
        .children(indicator);
    choice
        .label
        .root()
        .flex()
        .items_center()
        .gap_2()
        .child(button)
        .child(choice.text)
}

fn group(group: &Group, cx: &App) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_sm().child(group.title))
        .child(
            RadioGroup::group(&group.entity, cx)
                .aria_label(SharedString::from(group.title))
                .flex()
                .flex_col()
                .gap_2()
                .children(group.choices.iter().map(|choice| radio(&group.entity, choice, cx))),
        )
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plan_value = self.plan_value.clone();
        self.plan.entity.update(cx, |group, cx| {
            group.set_controlled_value(Some(plan_value), window, cx)
        });
        let cx: &App = cx;
        div()
            .size_full()
            .flex()
            .gap_8()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(group(&self.shipping, cx))
            .child(group(&self.plan, cx))
            .child(group(&self.backups, cx))
            .child(group(&self.theme, cx))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(520.), px(180.)), cx);
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
