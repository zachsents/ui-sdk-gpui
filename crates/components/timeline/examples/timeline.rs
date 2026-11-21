//! Timelines: a project's activity, a horizontal roadmap that stops at its
//! ends, an order's progress with its current step and a button that jumps
//! to it, and a feed that's busy while it loads more.

use std::time::Duration;

use gpui::{
    App, AppContext, Bounds, Context, Div, Entity, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Task, Window, WindowBounds,
    WindowOptions, div, px, rgb, size, transparent_black,
};
use ui_sdk_timeline::{
    Timeline, TimelineItemState, TimelineOptions, TimelineOrientation, TimelineStatus,
};

struct Event {
    value: SharedString,
    title: SharedString,
    time: SharedString,
    description: Option<SharedString>,
    status: Option<TimelineStatus>,
}

impl Event {
    fn new(value: &str, title: &str, time: &str) -> Self {
        Self {
            value: value.to_string().into(),
            title: title.to_string().into(),
            time: time.to_string().into(),
            description: None,
            status: None,
        }
    }

    fn description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string().into());
        self
    }

    fn status(mut self, status: TimelineStatus) -> Self {
        self.status = Some(status);
        self
    }
}

struct Example {
    activity: Entity<Timeline>,
    activity_events: Vec<Event>,
    roadmap: Entity<Timeline>,
    milestones: Vec<Event>,
    order: Entity<Timeline>,
    steps: Vec<Event>,
    feed: Entity<Timeline>,
    updates: Vec<Event>,
    loading: Option<Task<()>>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut timeline = |options: TimelineOptions| cx.new(|cx| Timeline::new(options, window, cx));
        let activity = timeline(TimelineOptions::default());
        let roadmap = timeline(
            TimelineOptions::default()
                .orientation(TimelineOrientation::Horizontal)
                .r#loop(false),
        );
        let order = timeline(TimelineOptions::default());
        let feed = timeline(TimelineOptions::default().feed(true));
        Self {
            activity,
            activity_events: vec![
                Event::new("deploy", "Deployed to production", "2:32 PM")
                    .description("Version 2.4.1 is live."),
                Event::new("review", "Review approved", "1:10 PM").description("By Mara Okafor."),
                Event::new("push", "Pushed 3 commits", "11:48 AM"),
                Event::new("open", "Opened pull request", "9:05 AM")
                    .description("Faster cold starts."),
            ],
            roadmap,
            milestones: vec![
                Event::new("research", "Research", "January"),
                Event::new("design", "Design", "March"),
                Event::new("beta", "Beta", "June"),
                Event::new("launch", "Launch", "September"),
            ],
            order,
            steps: vec![
                Event::new("ordered", "Ordered", "Nov 2").status(TimelineStatus::Complete),
                Event::new("packed", "Packed", "Nov 3").status(TimelineStatus::Complete),
                Event::new("shipped", "Shipped, in progress", "Nov 4")
                    .status(TimelineStatus::Current),
                Event::new("delivered", "Delivered", "Nov 7").status(TimelineStatus::Upcoming),
            ],
            feed,
            updates: (1..=3).map(update).collect(),
            loading: None,
        }
    }

    // Busy until three more updates arrive.
    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading.is_some() {
            return;
        }
        self.loading = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(900)).await;
            this.update(cx, |example, cx| {
                let next = example.updates.len() + 1;
                example.updates.extend((next..next + 3).map(update));
                example.loading = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

fn update(n: usize) -> Event {
    Event::new(&format!("update-{n}"), &format!("Update {n}"), &format!("{n}h ago"))
        .description("Release notes")
}

fn status_color(status: Option<TimelineStatus>) -> Hsla {
    match status {
        Some(TimelineStatus::Complete) => rgb(0x16a34a).into(),
        Some(TimelineStatus::Current) => rgb(0x2563eb).into(),
        Some(TimelineStatus::Upcoming) | None => rgb(0x9ca3af).into(),
    }
}

fn item_style(el: Stateful<Div>, _state: &TimelineItemState) -> Stateful<Div> {
    el.p_2()
        .rounded_md()
        .border_2()
        .border_color(transparent_black())
        .focus_visible(|style| style.border_color(rgb(0x2563eb)))
}

// One event: the marker and the connector to the next, then its title, time
// and description. The item comes first, since the parts inside read its
// status.
fn event(entity: &Entity<Timeline>, event: &Event, horizontal: bool, cx: &App) -> impl IntoElement {
    let value = event.value.clone();
    let item = Timeline::item(entity, value.clone(), event.status, cx).with(item_style);
    let item = if horizontal {
        item.flex().flex_col().gap_2().w(px(130.))
    } else {
        item.flex().gap_3()
    };
    let marker = Timeline::marker(entity, value.clone(), cx)
        .with(|el, state| el.bg(status_color(state.status)))
        .size(px(10.))
        .flex_shrink_0()
        .rounded_full();
    let connector = Timeline::connector(entity, value.clone(), cx).with(|el, state| {
        let el = el.bg(rgb(0xe5e7eb)).flex_1();
        let el = if horizontal { el.h(px(2.)) } else { el.w(px(2.)) };
        if state.last { el.invisible() } else { el }
    });
    let line = if horizontal {
        div().flex().items_center().gap_1().w_full()
    } else {
        div().flex().flex_col().items_center().gap_1().pt_1()
    };
    let text = div()
        .flex()
        .flex_col()
        .child(
            Timeline::title(entity, value.clone(), cx)
                .aria_label(event.title.clone())
                .child(event.title.clone()),
        )
        .child(
            Timeline::time(entity, value.clone(), cx)
                .aria_label(event.time.clone())
                .text_color(rgb(0x6b7280))
                .child(event.time.clone()),
        )
        .children(event.description.clone().map(|description| {
            Timeline::description(entity, value.clone(), cx)
                .aria_label(description.clone())
                .child(description)
        }));
    item.child(line.child(marker).child(connector)).child(text)
}

fn timeline(entity: &Entity<Timeline>, label: &'static str, events: &[Event], cx: &App) -> impl IntoElement {
    let horizontal = entity.read(cx).state().orientation == TimelineOrientation::Horizontal;
    let root = Timeline::root(entity, cx).aria_label(label);
    let root = if horizontal {
        root.flex()
    } else {
        root.flex().flex_col().w(px(300.))
    };
    root.children(events.iter().map(|each| event(entity, each, horizontal, cx)))
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
        let busy = self.loading.is_some();
        self.feed.update(cx, |feed, cx| {
            feed.set_options(TimelineOptions::default().feed(true).busy(busy), cx)
        });
        let current = button("current", "Go to current step").on_click(cx.listener(|this, _, window, cx| {
            this.order.update(cx, |order, cx| order.focus(Some("shipped"), window, cx));
        }));
        let more = button("more", if busy { "Loading" } else { "Load more" })
            .on_click(cx.listener(|this, _, _window, cx| this.load_more(cx)));
        let cx: &App = cx;
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_6()
            .p_6()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(timeline(&self.roadmap, "Roadmap", &self.milestones, cx))
            .child(
                div()
                    .flex()
                    .gap_8()
                    .child(timeline(&self.activity, "Activity", &self.activity_events, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(timeline(&self.order, "Order status", &self.steps, cx))
                            .child(current),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(timeline(&self.feed, "Updates", &self.updates, cx))
                            .child(more),
                    ),
            )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        ui_sdk_focus::bind_tab_keys(cx);
        let bounds = Bounds::centered(None, size(px(1040.), px(720.)), cx);
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
