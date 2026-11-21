//! The agent bridge on a few live components: a panel shows what an agent
//! reads, runs a step on a component by its ref or label, and lists every
//! change as it comes. The panel is marked hidden, so the agent never sees
//! it, and Delete project needs a person, whom `allow` asks.

use futures::StreamExt;
use futures::channel::mpsc::unbounded;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Div, Entity, InteractiveElement, IntoElement, ParentElement,
    PromptLevel, Render, SharedString, Stateful, StatefulInteractiveElement, Styled, Subscription, Task, Window,
    WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_button::{Button, ButtonOptions, ButtonState};
use ui_sdk_input::{Input, InputOptions, TextFieldKind};
use ui_sdk_kernel::{ActionOutcome, AgentValue, Part};
use ui_sdk_native::{TextField, TextFieldOptions, TextFieldState};
use ui_sdk_page::{
    ActOptions, AgentBridge, AgentMarks, AgentStep, Allowed, CreateAgentOptions, components_value, create_agent,
    with_agent_marks,
};
use ui_sdk_select::{
    ListData, ListItem, Select, SelectContentProps, SelectItemProps, SelectOptions, SelectTriggerProps,
    SelectValueProps,
};
use ui_sdk_slider::{Slider, SliderOptions};
use ui_sdk_switch::{Switch, SwitchOptions};

const REGIONS: &[(&str, &str)] = &[
    ("europe", "Europe"),
    ("north-america", "North America"),
    ("asia-pacific", "Asia Pacific"),
];

struct Example {
    agent: AgentBridge,
    project: Entity<Input>,
    password: Entity<Input>,
    region: Entity<Select>,
    notifications: Entity<Switch>,
    volume: Entity<Slider<f64>>,
    save: Entity<Button>,
    delete: Entity<Button>,
    step_ref: Entity<TextField>,
    step_action: Entity<TextField>,
    step_args: Entity<TextField>,
    output: SharedString,
    changes: Vec<SharedString>,
    _changes: Subscription,
    _listening: Task<()>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A step on something marked for a person asks them.
        let agent = create_agent(
            CreateAgentOptions::default().allow(|step, _context, window, cx| match &step.confirm {
                None => true.into(),
                Some(confirm) => {
                    let answer = window.prompt(PromptLevel::Warning, confirm, None, &["Allow", "Deny"], cx);
                    Allowed::later(async move { answer.await == Ok(0) })
                }
            }),
            cx,
        );
        let project = cx.new(|cx| Input::new(InputOptions::default().default_value("Apollo"), window, cx));
        // A password reads as "[redacted]" to the agent.
        let password = cx.new(|cx| {
            Input::new(
                InputOptions::default()
                    .kind(TextFieldKind::Password)
                    .default_value("correct horse"),
                window,
                cx,
            )
        });
        let region = cx.new(|cx| {
            Select::new(
                SelectOptions::default()
                    .placeholder("Choose one")
                    .items(ListData::new(
                        REGIONS
                            .iter()
                            .map(|&(value, label)| ListItem::new(SharedString::from(value), label)),
                    )),
                window,
                cx,
            )
        });
        let notifications = cx.new(|cx| Switch::new(SwitchOptions::default().default_checked(true), window, cx));
        let volume = cx.new(|cx| Slider::new(SliderOptions::default().default_value(40.0), window, cx));
        let save = cx.new(|cx| {
            Button::new(
                ButtonOptions::default().action(|_event, details, _window, _cx| {
                    log::info!("saved ({})", details.source.as_str());
                    ActionOutcome::Done
                }),
                window,
                cx,
            )
        });
        let delete = cx.new(|cx| {
            Button::new(
                ButtonOptions::default().action(|_event, details, _window, _cx| {
                    log::info!("deleted ({})", details.source.as_str());
                    ActionOutcome::Done
                }),
                window,
                cx,
            )
        });
        let mut field = |placeholder: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| TextField::new(TextFieldOptions::default().placeholder(placeholder), window, cx))
        };
        let step_ref = field("Region", cx);
        let step_action = field("setValue", cx);
        let step_args = field("[\"Asia Pacific\"]", cx);
        // Changes reach a listener inside the component's update, so they
        // come to the view through a channel.
        let (sender, mut receiver) = unbounded::<SharedString>();
        let changes = agent.subscribe(
            move |change| {
                let line = format!(
                    "{} {} {}: {} → {} ({}, {})",
                    change.r#ref,
                    change.label.as_deref().unwrap_or(&change.component),
                    change.key,
                    change.previous,
                    change.value,
                    change.reason.as_str(),
                    change.source.as_str()
                );
                sender.unbounded_send(line.into()).ok();
            },
            cx,
        );
        let listening = cx.spawn(async move |this, cx| {
            while let Some(line) = receiver.next().await {
                this.update(cx, |example, cx| {
                    example.changes.insert(0, line);
                    example.changes.truncate(12);
                    cx.notify();
                })
                .ok();
            }
        });
        Self {
            agent,
            project,
            password,
            region,
            notifications,
            volume,
            save,
            delete,
            step_ref,
            step_action,
            step_args,
            output: SharedString::default(),
            changes: Vec::new(),
            _changes: changes,
            _listening: listening,
        }
    }

    fn snapshot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let components = self.agent.snapshot(window, cx);
        self.output = pretty(&components_value(&components));
        cx.notify();
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let text = |field: &Entity<TextField>, cx: &Context<Self>| field.read(cx).value().trim().to_string();
        let args = text(&self.step_args, cx);
        // Arguments as JSON, a list or one value, or else as text.
        let args = match serde_json::from_str::<AgentValue>(&args) {
            _ if args.is_empty() => Vec::new(),
            Ok(AgentValue::Array(args)) => args,
            Ok(arg) => vec![arg],
            Err(_) => vec![AgentValue::String(args)],
        };
        let step = AgentStep::new(text(&self.step_ref, cx), text(&self.step_action, cx)).args(args);
        let task = self.agent.act(vec![step], ActOptions::default(), cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |example, cx| {
                example.output = pretty(&result.to_value());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn pretty(value: &AgentValue) -> SharedString {
    serde_json::to_string_pretty(value).unwrap_or_default().into()
}

fn button_look(el: Stateful<Div>, state: &ButtonState) -> Stateful<Div> {
    el.bg(if state.disabled { rgb(0xe5e7eb) } else { rgb(0xdbeafe) })
}

fn button(entity: &Entity<Button>, label: &'static str, cx: &App) -> Part<ButtonState> {
    Button::root(entity, cx)
        .aria_label(SharedString::from(label))
        .with(button_look)
        .px_3()
        .py_1()
        .rounded_md()
        .child(label)
}

fn panel_button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(0xf3f4f6))
        .hover(|style| style.bg(rgb(0xe5e7eb)))
        .child(label)
}

fn labeled(label: &'static str, field: Part<TextFieldState>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_sm().text_color(rgb(0x4b5563)).child(label))
        .child(
            field
                .aria_label(label)
                .w(px(240.))
                .border_1()
                .border_color(rgb(0xd1d5db))
                .rounded_md()
                .px_2()
                .py_1(),
        )
}

impl Example {
    fn region(&self, cx: &App) -> impl IntoElement {
        let items: Vec<AnyElement> = REGIONS
            .iter()
            .map(|&(value, label)| {
                Select::item(&self.region, SelectItemProps::new(value).text(label), cx)
                    .aria_label(label)
                    .with(|el, state| el.when(state.highlighted, |el| el.bg(rgb(0xeef2ff))))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .child(label)
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Select::label(&self.region, cx).aria_label("Region").child("Region"))
            .child(
                Select::trigger(&self.region, SelectTriggerProps::default(), cx)
                    .flex()
                    .justify_between()
                    .w(px(200.))
                    .px_3()
                    .py_1()
                    .border_1()
                    .border_color(rgb(0xd1d5db))
                    .rounded_md()
                    .child(
                        Select::value_part(&self.region, SelectValueProps::default(), cx)
                            .with(|el, state| el.child(state.text.clone())),
                    )
                    .child("▾"),
            )
            .children(Select::content(&self.region, SelectContentProps::default(), cx).map(|content| {
                content
                    .flex()
                    .flex_col()
                    .min_w(px(200.))
                    .p_1()
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xd1d5db))
                    .rounded_md()
                    .children(items)
                    .into_any_element()
            }))
    }
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let switch = Switch::root(&self.notifications, cx)
            .aria_label("Notifications")
            .with(|el, state| {
                el.bg(if state.checked { rgb(0x2563eb) } else { rgb(0x9ca3af) })
                    .when(state.checked, |el| el.justify_end())
            })
            .flex()
            .w(px(36.))
            .p(px(2.))
            .rounded_full()
            .child(Switch::thumb(&self.notifications, cx).size(px(16.)).rounded_full().bg(rgb(0xffffff)));
        let volume = Slider::root(&self.volume, cx).child(
            Slider::control(&self.volume, cx)
                .flex()
                .items_center()
                .w(px(200.))
                .h(px(20.))
                .child(
                    Slider::track(&self.volume, cx)
                        .h(px(4.))
                        .w_full()
                        .rounded_full()
                        .bg(rgb(0xe5e7eb))
                        .child(Slider::indicator(&self.volume, cx).rounded_full().bg(rgb(0x1f2937)))
                        .child(
                            Slider::thumb(&self.volume, 0, cx)
                                .aria_label("Volume")
                                .size_4()
                                .rounded_full()
                                .border_2()
                                .border_color(rgb(0x1f2937))
                                .bg(rgb(0xffffff)),
                        ),
                ),
        );
        let page = div()
            .flex()
            .flex_col()
            .items_start()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().child("Project name"))
                    .child(
                        Input::root(&self.project, cx)
                            .aria_label("Project name")
                            .w(px(200.))
                            .border_1()
                            .border_color(rgb(0xd1d5db))
                            .rounded_md()
                            .px_2()
                            .py_1(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().child("Password"))
                    .child(
                        Input::root(&self.password, cx)
                            .aria_label("Password")
                            .w(px(200.))
                            .border_1()
                            .border_color(rgb(0xd1d5db))
                            .rounded_md()
                            .px_2()
                            .py_1(),
                    ),
            )
            .child(self.region(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(switch)
                    .child("Notifications"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().child("Volume"))
                    .child(volume),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(button(&self.save, "Save", cx))
                    .child(
                        button(&self.delete, "Delete project", cx)
                            .agent_marks(AgentMarks::new().confirm("Deletes the project for everyone")),
                    ),
            );
        let panel = div()
            .id("agent-panel")
            .flex()
            .flex_col()
            .gap_3()
            .w(px(440.))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_2()
                    .child(labeled("Ref", TextField::root(&self.step_ref, cx)))
                    .child(
                        panel_button("snapshot", "Snapshot")
                            .on_click(cx.listener(|this, _, window, cx| this.snapshot(window, cx))),
                    ),
            )
            .child(labeled("Action", TextField::root(&self.step_action, cx)))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_2()
                    .child(labeled("Args", TextField::root(&self.step_args, cx)))
                    .child(panel_button("run", "Run").on_click(cx.listener(|this, _, _window, cx| this.run(cx)))),
            )
            .child(
                div()
                    .id("output")
                    .h(px(300.))
                    .overflow_y_scroll()
                    .p_2()
                    .rounded_md()
                    .bg(rgb(0xf9fafb))
                    .text_xs()
                    .child(self.output.clone()),
            )
            .child(div().text_sm().text_color(rgb(0x4b5563)).child("Changes"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .text_xs()
                    .children(self.changes.iter().cloned()),
            );
        div()
            .size_full()
            .flex()
            .items_start()
            .gap_8()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(page)
            .child(with_agent_marks(panel, AgentMarks::new().hidden(true)))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(800.), px(560.)), cx);
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
