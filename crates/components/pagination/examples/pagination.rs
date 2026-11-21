//! Paginations: ten pages with Previous and Next, ninety-five rows ten or
//! twenty-five to a page with First and Last, links a router follows, twenty
//! pages with two siblings and two boundaries, and a disabled one.

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Div, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Stateful, StatefulInteractiveElement, Styled, WeakEntity, Window,
    WindowBounds, WindowOptions, div, px, rgb, size,
};
use ui_sdk_kernel::{ChangeOptions, Part};
use ui_sdk_pagination::{
    Pagination, PaginationLinkState, PaginationOptions, PaginationRangeItem, PaginationStepState,
};

const ROWS: usize = 95;

struct Example {
    basic: Entity<Pagination>,
    change: Option<(usize, &'static str)>,
    rows: Entity<Pagination>,
    page: usize,
    page_size: usize,
    links: Entity<Pagination>,
    route: SharedString,
    wide: Entity<Pagination>,
    disabled: Entity<Pagination>,
}

// The page a route like "/posts?page=3" names.
fn page_of(route: &str) -> usize {
    route
        .rsplit_once("page=")
        .and_then(|(_, page)| page.parse().ok())
        .unwrap_or(1)
}

fn rows(this: WeakEntity<Example>, page: usize, page_size: usize) -> PaginationOptions {
    PaginationOptions::default()
        .total(ROWS)
        .page_size(page_size)
        .page(page)
        .on_page_change(move |page, _details, _window, cx| {
            let page = *page;
            this.update(cx, |example, cx| {
                example.page = page;
                cx.notify();
            })
            .ok();
        })
}

fn links(route: &str) -> PaginationOptions {
    PaginationOptions::default()
        .page_count(10)
        .page(page_of(route))
        .get_href(|page| format!("/posts?page={page}").into())
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        let logged = this.clone();
        let basic = cx.new(|cx| {
            Pagination::new(
                PaginationOptions::default()
                    .page_count(10)
                    .on_page_change(move |page, details, _window, cx| {
                        let change = (*page, details.reason.as_str());
                        logged
                            .update(cx, |example, cx| {
                                example.change = Some(change);
                                cx.notify();
                            })
                            .ok();
                    }),
                window,
                cx,
            )
        });
        let route: SharedString = "/posts?page=1".into();
        let rows = cx.new(|cx| Pagination::new(rows(this, 1, 10), window, cx));
        let links = cx.new(|cx| Pagination::new(links(&route), window, cx));
        let wide = cx.new(|cx| {
            Pagination::new(
                PaginationOptions::default()
                    .page_count(20)
                    .default_page(10)
                    .siblings(2)
                    .boundaries(2),
                window,
                cx,
            )
        });
        let disabled = cx.new(|cx| {
            Pagination::new(
                PaginationOptions::default()
                    .page_count(5)
                    .default_page(2)
                    .disabled(true),
                window,
                cx,
            )
        });
        Self {
            basic,
            change: None,
            rows,
            page: 1,
            page_size: 10,
            links,
            route,
            wide,
            disabled,
        }
    }

    // A new page size starts again at page 1.
    fn set_page_size(&mut self, page_size: usize, cx: &mut Context<Self>) {
        self.page_size = page_size;
        self.page = 1;
        cx.notify();
    }

    // What a router does when a link it renders is followed.
    fn navigate(&mut self, href: SharedString, cx: &mut Context<Self>) {
        self.route = href;
        cx.notify();
    }
}

fn link_style(el: Stateful<Div>, state: &PaginationLinkState) -> Stateful<Div> {
    let el = el.min_w(px(28.)).flex().justify_center().px_2().py_1().rounded_md();
    match (state.current, state.disabled) {
        (_, true) => el.text_color(rgb(0x9ca3af)),
        (true, false) => el.bg(rgb(0x1f2937)).text_color(rgb(0xffffff)),
        (false, false) => el.bg(rgb(0xf3f4f6)),
    }
}

fn step(part: Part<PaginationStepState>, label: &'static str) -> Part<PaginationStepState> {
    part.aria_label(label)
        .with(|el, state| {
            el.text_color(if state.disabled {
                rgb(0x9ca3af)
            } else {
                rgb(0x111827)
            })
        })
        .px_2()
        .py_1()
        .rounded_md()
        .child(label)
}

// What a router's link adds: following the href, unless the pagination
// prevented it, like at an end or for a canceled change, or a modifier key
// opens it elsewhere.
fn follows<S: 'static>(
    part: Part<S>,
    href: Option<SharedString>,
    router: &WeakEntity<Example>,
) -> Part<S> {
    let router = router.clone();
    part.on_click(move |event, window, cx| {
        if window.default_prevented() || event.modifiers().modified() {
            return;
        }
        if let Some(href) = href.clone() {
            router.update(cx, |example, cx| example.navigate(href, cx)).ok();
        }
    })
}

fn pagination(
    entity: &Entity<Pagination>,
    label: &'static str,
    ends: bool,
    router: Option<&WeakEntity<Example>>,
    cx: &App,
) -> impl IntoElement {
    let list = Pagination::list(entity, cx);
    let items = list.state().items.clone();
    let mut parts: Vec<AnyElement> = Vec::new();
    let step_item = |key: &'static str, part: Part<PaginationStepState>, label: &'static str| {
        let href = part.state().href.clone();
        let part = step(part, label);
        let part = match router {
            Some(router) => follows(part, href, router),
            None => part,
        };
        Pagination::item(entity, key, cx).child(part).into_any_element()
    };
    if ends {
        parts.push(step_item("first", Pagination::first_part(entity, cx), "First"));
    }
    parts.push(step_item("previous", Pagination::previous_part(entity, cx), "Previous"));
    let mut last = Vec::new();
    last.push(step_item("next", Pagination::next_part(entity, cx), "Next"));
    if ends {
        last.push(step_item("last", Pagination::last_part(entity, cx), "Last"));
    }
    for item in items {
        let key = item.key();
        let content = match item {
            PaginationRangeItem::Page(page) => {
                let link = Pagination::link(entity, page, cx);
                let href = link.state().href.clone();
                let link = link
                    .aria_label(SharedString::from(page.to_string()))
                    .with(link_style)
                    .child(page.to_string());
                match router {
                    Some(router) => follows(link, href, router).into_any_element(),
                    None => link.into_any_element(),
                }
            }
            PaginationRangeItem::EllipsisStart | PaginationRangeItem::EllipsisEnd => {
                Pagination::ellipsis(entity, key.clone(), cx)
                    .px_1()
                    .child("…")
                    .into_any_element()
            }
        };
        parts.push(Pagination::item(entity, key, cx).child(content).into_any_element());
    }
    parts.extend(last);
    Pagination::root(entity, cx)
        .aria_label(label)
        .child(list.flex().items_center().gap_1().children(parts))
}

fn button(id: impl Into<SharedString>, label: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    div()
        .id(gpui::ElementId::Name(id.into()))
        .px_2()
        .py_1()
        .rounded_md()
        .bg(if selected { rgb(0x1f2937) } else { rgb(0xf3f4f6) })
        .text_color(if selected { rgb(0xffffff) } else { rgb(0x111827) })
        .child(label.into())
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let options = rows(this.clone(), self.page, self.page_size);
        self.rows.update(cx, |pagination, cx| pagination.set_options(options, cx));
        let options = links(&self.route);
        self.links.update(cx, |pagination, cx| pagination.set_options(options, cx));

        let next = button("next-page", "Next page", false).on_click(cx.listener(|this, _, window, cx| {
            this.basic.update(cx, |pagination, cx| {
                pagination.next(ChangeOptions::default(), window, cx);
            });
        }));
        let sizes = [10, 25].map(|page_size| {
            button(format!("size-{page_size}"), format!("{page_size} rows"), self.page_size == page_size)
                .on_click(cx.listener(move |this, _, _window, cx| this.set_page_size(page_size, cx)))
        });
        let first_row = (self.page - 1) * self.page_size + 1;
        let last_row = (self.page * self.page_size).min(ROWS);
        let change = match self.change {
            Some((page, reason)) => format!("{page} · {reason}"),
            None => String::new(),
        };
        let route = self.route.clone();
        let cx: &App = cx;
        let row = || div().flex().items_center().gap_4();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_6()
            .p_6()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x111827))
            .text_sm()
            .child(
                row()
                    .child(pagination(&self.basic, "Results", false, None, cx))
                    .child(next)
                    .child(div().text_color(rgb(0x6b7280)).child(change)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(pagination(&self.rows, "Rows", true, None, cx))
                    .child(
                        row()
                            .child(div().flex().gap_1().children(sizes))
                            .child(
                                div()
                                    .text_color(rgb(0x6b7280))
                                    .child(format!("{first_row}–{last_row} of {ROWS}")),
                            ),
                    ),
            )
            .child(
                row()
                    .child(pagination(&self.links, "Posts", false, Some(&this), cx))
                    .child(div().text_color(rgb(0x6b7280)).child(route)),
            )
            .child(pagination(&self.wide, "Archive", true, None, cx))
            .child(pagination(&self.disabled, "Loading", false, None, cx))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        ui_sdk_focus::bind_tab_keys(cx);
        let bounds = Bounds::centered(None, size(px(760.), px(420.)), cx);
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
