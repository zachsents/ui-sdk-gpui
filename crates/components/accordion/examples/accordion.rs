//! Accordions: one item at a time with a disabled item, one that always keeps
//! an item open, and several at once, animated, with Expand all.

use gpui::{
    App, AppContext, Bounds, Context, Div, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Stateful, StatefulInteractiveElement, Styled, Window, WindowBounds,
    WindowOptions, div, ease_in_out, px, rgb, size,
};
use ui_sdk_accordion::{
    Accordion, AccordionItemProps, AccordionOptions, AccordionPanelProps, AccordionPanelState,
    AccordionTriggerProps,
};
use ui_sdk_kernel::ChangeOptions;
use ui_sdk_motion::{PresenceTiming, Timing};

const FAQ: [(&str, &str, &str); 3] = [
    ("shipping", "Shipping", "Five to seven days"),
    ("returns", "Returns", "Thirty days"),
    ("warranty", "Warranty", "Two years"),
];

const COURSE: [(&str, &str, &str); 3] = [
    ("overview", "Overview", "Six weeks"),
    ("timeline", "Timeline", "Mondays, 6 pm"),
    ("grading", "Grading", "Pass or fail"),
];

const SETTINGS: [(&str, &str, &str); 3] = [
    ("notifications", "Notifications", "Email and push"),
    ("privacy", "Privacy and security", "Two-factor on"),
    ("billing", "Billing", "Visa ending 4242"),
];

struct Example {
    faq: Entity<Accordion>,
    course: Entity<Accordion>,
    settings: Entity<Accordion>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let faq = cx.new(|cx| Accordion::new(AccordionOptions::default().default_value(["shipping"]), window, cx));
        let course = cx.new(|cx| {
            Accordion::new(
                AccordionOptions::default().collapsible(false).default_value(["overview"]),
                window,
                cx,
            )
        });
        let settings = cx.new(|cx| {
            Accordion::new(
                AccordionOptions::default()
                    .multiple(true)
                    .panel_timing(PresenceTiming::new(Timing::ms(150).easing(ease_in_out))),
                window,
                cx,
            )
        });
        Self { faq, course, settings }
    }
}

fn panel_style(el: Stateful<Div>, state: &AccordionPanelState) -> Stateful<Div> {
    let el = el.px_2().py_1();
    match state.panel.height {
        Some(height) => el.h(height).overflow_hidden(),
        None => el,
    }
}

fn accordion(
    entity: &Entity<Accordion>,
    items: &[(&'static str, &'static str, &'static str)],
    disabled: Option<&str>,
    cx: &App,
) -> impl IntoElement {
    Accordion::root(entity, cx)
        .flex()
        .flex_col()
        .gap_1()
        .w(px(280.))
        .children(items.iter().map(|&(value, title, body)| {
            let props = AccordionItemProps::default().disabled(disabled == Some(value));
            Accordion::item(entity, value, props, cx)
                .flex()
                .flex_col()
                .child(
                    Accordion::header(entity, value, cx).child(
                        Accordion::trigger(entity, value, AccordionTriggerProps::default(), cx)
                            .aria_label(SharedString::from(title))
                            .with(|el, state| {
                                el.bg(if state.open { rgb(0xe5e7eb) } else { rgb(0xf3f4f6) })
                                    .text_color(if state.disabled { rgb(0x9ca3af) } else { rgb(0x111827) })
                            })
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .child(title),
                    ),
                )
                .children(
                    Accordion::panel(entity, value, AccordionPanelProps::default(), cx)
                        .map(|panel| panel.with(panel_style).child(body)),
                )
        }))
}

fn button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(0xf3f4f6))
        .child(label)
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let expand = button("expand", "Expand all").on_click(cx.listener(|this, _, window, cx| {
            this.settings.update(cx, |settings, cx| {
                settings.open_all(ChangeOptions::default(), window, cx);
            });
        }));
        let collapse = button("collapse", "Collapse all").on_click(cx.listener(|this, _, window, cx| {
            this.settings.update(cx, |settings, cx| {
                settings.close_all(ChangeOptions::default(), window, cx);
            });
        }));
        let cx: &App = cx;
        div()
            .size_full()
            .flex()
            .gap_4()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .child(accordion(&self.faq, &FAQ, Some("warranty"), cx))
            .child(accordion(&self.course, &COURSE, None, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().flex().gap_2().child(expand).child(collapse))
                    .child(accordion(&self.settings, &SETTINGS, None, cx)),
            )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(940.), px(360.)), cx);
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
