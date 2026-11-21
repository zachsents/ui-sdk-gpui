//! Trees: files from paths that open on Enter, mail folders with several
//! selected, a stack with checkboxes, a database whose branches load, and
//! pages to drag into a new order and rename in place.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Div, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Subscription, Window, WindowBounds, WindowOptions, div, prelude::FluentBuilder, px, rgb, size,
    transparent_black,
};
use ui_sdk_drag_and_drop::{
    TreeReorder, TreeReorderItem, TreeReorderOptions, TreeReorderRow, create_tree_reorder,
    on_pointer_down,
};
use ui_sdk_kernel::modality::focus_visible;
use ui_sdk_kernel::{ActionOutcome, ChangeOptions, Checked, InputEvent, Key, Reason};
use ui_sdk_lists::{InlineEdit, InlineEditOptions, InlineEditReason, create_inline_edit};
use ui_sdk_native::{TextField, TextFieldEvent, TextFieldOptions};
use ui_sdk_tree::{
    Tree, TreeActivation, TreeCheckStrategy, TreeGroupProps, TreeItemData, TreeItemProps,
    TreeItemState, TreeMode, TreeOptions, TreeValue,
};

const FILES: &[&str] = &[
    "app/layout.tsx",
    "app/page.tsx",
    "app/settings/page.tsx",
    "components/button.tsx",
    "components/card.tsx",
    "components.json",
    "package.json",
    "README.md",
];

fn mail() -> Vec<TreeItemData> {
    vec![
        TreeItemData::new("inbox", "Inbox").children([
            TreeItemData::new("receipts", "Receipts"),
            TreeItemData::new("travel", "Travel"),
        ]),
        TreeItemData::new("projects", "Projects").children([
            TreeItemData::new("atlas", "Atlas"),
            TreeItemData::new("beacon", "Beacon"),
        ]),
        TreeItemData::new("archive", "Archive"),
        TreeItemData::new("sent", "Sent"),
    ]
}

fn stack() -> Vec<TreeItemData> {
    vec![
        TreeItemData::new("frontend", "Frontend").children([
            TreeItemData::new("react", "React"),
            TreeItemData::new("vue", "Vue"),
            TreeItemData::new("svelte", "Svelte"),
        ]),
        TreeItemData::new("backend", "Backend").children([
            TreeItemData::new("node", "Node"),
            TreeItemData::new("databases", "Databases").children([
                TreeItemData::new("postgres", "Postgres"),
                TreeItemData::new("sqlite", "SQLite"),
            ]),
        ]),
    ]
}

const PAGES: &[(&str, &str, usize)] = &[
    ("start", "Getting started", 0),
    ("install", "Install", 1),
    ("quick", "Quick start", 1),
    ("guides", "Guides", 0),
    ("forms", "Forms", 1),
    ("validation", "Validation", 2),
    ("actions", "Server actions", 2),
    ("dialogs", "Dialogs", 1),
    ("dnd", "Drag and drop", 1),
    ("reference", "Reference", 0),
    ("core", "Core", 1),
    ("react", "React", 1),
    ("changelog", "Changelog", 0),
];

const TABLES: &[&str] = &["users", "orders", "archive"];

// Nested pages from rows in reading order, each under the row above it that
// sits a level up.
fn pages_from_rows(rows: &[TreeReorderRow<SharedString>], names: &HashMap<SharedString, SharedString>) -> Vec<TreeItemData> {
    fn take(
        rows: &[TreeReorderRow<SharedString>],
        at: &mut usize,
        depth: usize,
        names: &HashMap<SharedString, SharedString>,
    ) -> Vec<TreeItemData> {
        let mut items = Vec::new();
        while *at < rows.len() && rows[*at].depth == depth {
            let key = rows[*at].key.clone();
            *at += 1;
            let mut item = TreeItemData::new(key.clone(), names.get(&key).cloned().unwrap_or(key));
            if *at < rows.len() && rows[*at].depth > depth {
                item = item.children(take(rows, at, depth + 1, names));
            }
            items.push(item);
        }
        items
    }
    take(rows, &mut 0, 0, names)
}

fn pages_options(items: Rc<[TreeItemData]>) -> TreeOptions {
    TreeOptions::default()
        .items(items)
        .mode(TreeMode::None)
        .default_expanded(["start", "guides", "forms"])
}

struct Example {
    files: Entity<Tree>,
    mail: Entity<Tree>,
    stack: Entity<Tree>,
    database: Entity<Tree>,
    tables: HashMap<SharedString, Vec<(SharedString, SharedString)>>,
    archive_failed: bool,
    pages: Entity<Tree>,
    page_rows: Vec<TreeReorderRow<SharedString>>,
    page_names: HashMap<SharedString, SharedString>,
    shown_rows: Vec<TreeReorderRow<SharedString>>,
    reorder: TreeReorder<Example, SharedString>,
    edit: InlineEdit<Example>,
    rename: Option<(Entity<TextField>, Subscription)>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let files = cx.new(|cx| {
            Tree::new(
                TreeOptions::default()
                    .paths(FILES.iter().copied())
                    .default_expanded(["app"])
                    .activation(TreeActivation::Automatic)
                    .on_action(|_value, _details, _window, cx| {
                        let opening = cx.background_executor().timer(Duration::from_millis(600));
                        ActionOutcome::wait(async move {
                            opening.await;
                            Ok::<(), String>(())
                        })
                    }),
                window,
                cx,
            )
        });
        let mail = cx.new(|cx| {
            Tree::new(
                TreeOptions::default()
                    .items(mail())
                    .mode(TreeMode::Multiple)
                    .expand_on_press(false)
                    .default_expanded(["inbox", "projects"])
                    .default_value(TreeValue::many(["inbox"])),
                window,
                cx,
            )
        });
        let stack = cx.new(|cx| {
            Tree::new(
                TreeOptions::default()
                    .items(stack())
                    .checkable(true)
                    .mode(TreeMode::None)
                    .check_strategy(TreeCheckStrategy::Trunk)
                    .default_checked(["react", "postgres"])
                    .default_expanded(["frontend", "backend", "databases"]),
                window,
                cx,
            )
        });
        let example = cx.entity().downgrade();
        let database = cx.new(|cx| {
            Tree::new(
                TreeOptions::default().load_children(move |value, details, _window, cx| {
                    let value = value.clone();
                    let signal = details.signal.clone();
                    let example = example.clone();
                    let wait = cx.background_executor().timer(Duration::from_millis(800));
                    ActionOutcome::wait(cx.spawn(async move |cx| {
                        wait.await;
                        if signal.aborted() {
                            return Err("Aborted".to_string());
                        }
                        example
                            .update(cx, |example, cx| example.load(&value, cx))
                            .map_err(|error| error.to_string())?
                    }))
                }),
                window,
                cx,
            )
        });
        let page_rows: Vec<TreeReorderRow<SharedString>> = PAGES
            .iter()
            .map(|(key, _, depth)| TreeReorderRow::new(SharedString::from(*key), *depth))
            .collect();
        let page_names: HashMap<SharedString, SharedString> = PAGES
            .iter()
            .map(|(key, name, _)| (SharedString::from(*key), SharedString::from(*name)))
            .collect();
        let items: Rc<[TreeItemData]> = pages_from_rows(&page_rows, &page_names).into();
        let pages = cx.new(|cx| Tree::new(pages_options(items), window, cx));
        let reorder = create_tree_reorder(
            TreeReorderOptions::new(
                |example: &Example| example.page_rows.clone(),
                |example: &mut Example, rows, place, details, window, cx| {
                    example.page_rows = rows;
                    if let Some(parent) = place.parent {
                        let options = ChangeOptions {
                            reason: Some(Reason::Drop),
                            event: details.event,
                            source: None,
                        };
                        example
                            .pages
                            .update(cx, |tree, cx| tree.expand(parent, options, window, cx));
                    }
                    cx.notify();
                    true
                },
            )
            .indent(|_| px(14.))
            .label(|example: &Example, key| example.page_names.get(key).cloned())
            .preview(true),
            cx,
        );
        let edit = create_inline_edit(InlineEditOptions::new(
            |example: &Example, id| example.page_names.get(id).cloned().unwrap_or_default(),
            |example: &mut Example, id, text, _details, _window, cx| {
                let name = text.trim();
                if name.is_empty() {
                    return false;
                }
                example.page_names.insert(id.clone(), SharedString::from(name.to_string()));
                // The pages render again, with the new name.
                example.shown_rows.clear();
                cx.notify();
                true
            },
        ));
        Self {
            files,
            mail,
            stack,
            database,
            tables: HashMap::new(),
            archive_failed: false,
            pages,
            shown_rows: page_rows.clone(),
            page_rows,
            page_names,
            reorder,
            edit,
            rename: None,
        }
    }

    // A rename started: its field takes the name and focus, and leaving it
    // saves.
    fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextField::new(TextFieldOptions::default(), window, cx));
        self.edit.bind_input(&input, None, window, cx);
        let blur = cx.subscribe_in(&input, window, |example, _input, event: &TextFieldEvent, window, cx| {
            if matches!(event, TextFieldEvent::Blur) {
                let edit = example.edit.clone();
                edit.end(example, InlineEditReason::FocusOut, None, window, cx);
            }
        });
        self.rename = Some((input, blur));
    }

    // A table's rows arrive; the archive fails the first time.
    fn load(&mut self, table: &SharedString, cx: &mut Context<Self>) -> Result<(), String> {
        if table == "archive" && !self.archive_failed {
            self.archive_failed = true;
            return Err("Network error".into());
        }
        let stem = table.trim_end_matches('s').to_string();
        let rows = (1..=3)
            .map(|n| {
                (
                    SharedString::from(format!("{table}/{n}")),
                    SharedString::from(format!("{stem}_{n}.json")),
                )
            })
            .collect();
        self.tables.insert(table.clone(), rows);
        cx.notify();
        Ok(())
    }
}

// A row: its item's label, indented by level, shaded when selected, ringed
// when it holds focus from the keyboard.
fn row(el: Stateful<Div>, state: &TreeItemState, focused: bool) -> Stateful<Div> {
    el.flex()
        .items_center()
        .gap_1()
        .h(px(24.))
        .pr_2()
        .pl(px(4. + (state.level as f32 - 1.) * 14.))
        .rounded_md()
        .border_1()
        .border_color(if focused { rgb(0x2563eb).into() } else { transparent_black() })
        .text_color(if state.disabled { rgb(0x9ca3af) } else { rgb(0x111827) })
        .when(state.selected, |el| el.bg(rgb(0xe5e7eb)))
        .when(state.pending, |el| el.text_color(rgb(0x6b7280)))
}

fn indicator(tree: &Entity<Tree>, value: &SharedString, cx: &App) -> impl IntoElement {
    Tree::indicator(tree, value.clone(), cx)
        .w(px(12.))
        .text_color(rgb(0x6b7280))
        .with(|el, state| {
            el.child(if state.loading {
                "…"
            } else if state.failed {
                "↻"
            } else if state.expanded {
                "▾"
            } else {
                "▸"
            })
        })
}

fn spacer() -> Div {
    div().w(px(12.))
}

fn checkbox(tree: &Entity<Tree>, value: &SharedString, cx: &App) -> impl IntoElement {
    Tree::checkbox(tree, value.clone(), cx).with(|el, state| {
        el.child(match state.checked {
            Checked::True => "☑",
            Checked::Indeterminate => "▣",
            Checked::False => "☐",
        })
    })
}

// A branch's group, with its children built only while it's in the page.
fn group(
    tree: &Entity<Tree>,
    value: &SharedString,
    cx: &App,
    children: impl FnOnce() -> Vec<AnyElement>,
) -> Option<AnyElement> {
    let group = Tree::group(tree, value.clone(), TreeGroupProps::default(), cx)?;
    Some(
        group
            .with(|el, state| {
                el.flex()
                    .flex_col()
                    .when_some(state.panel.height, |el, height| el.h(height).overflow_hidden())
            })
            .children(children())
            .into_any_element(),
    )
}

// The rows of a tree with data: the data names each item and says which
// are branches. Each item comes before its label and its group, which read
// what it rendered with.
fn data_rows(
    tree: &Entity<Tree>,
    items: &[TreeItemData],
    focused: Option<&SharedString>,
    cx: &App,
) -> Vec<AnyElement> {
    let checkable = tree.read(cx).state().checkable;
    items
        .iter()
        .map(|item| {
            let value = item.value.clone();
            let ringed = focused == Some(&value);
            let part = Tree::item(tree, value.clone(), TreeItemProps::default(), cx);
            let label = Tree::label(tree, value.clone(), cx)
                .with(|el, state| row(el, state, ringed))
                .map_el(|el| match item.children {
                    Some(_) => el.child(indicator(tree, &value, cx)),
                    None => el.child(spacer()),
                })
                .when(checkable, |el| el.child(checkbox(tree, &value, cx)))
                .child(item.label.clone());
            let group = item.children.as_ref().and_then(|children| {
                group(tree, &value, cx, || data_rows(tree, children, focused, cx))
            });
            part.child(label).children(group).into_any_element()
        })
        .collect()
}

fn titled(title: &'static str, tree: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .w(px(196.))
        .child(div().text_sm().text_color(rgb(0x6b7280)).child(title))
        .child(tree)
}

impl Example {
    fn database_rows(&self, focused: Option<&SharedString>, cx: &App) -> Vec<AnyElement> {
        let tree = &self.database;
        TABLES
            .iter()
            .map(|table| {
                let value = SharedString::from(*table);
                let ringed = focused == Some(&value);
                let props = TreeItemProps::new().branch(true).label(value.clone());
                let part = Tree::item(tree, value.clone(), props, cx);
                let label = Tree::label(tree, value.clone(), cx)
                    .with(|el, state| row(el, state, ringed))
                    .child(indicator(tree, &value, cx))
                    .child(value.clone());
                let rows = self.tables.get(&value);
                let group = rows.and_then(|rows| {
                    group(tree, &value, cx, || {
                        rows.iter()
                            .map(|(row_value, name)| {
                                let ringed = focused == Some(row_value);
                                let props = TreeItemProps::new().parent(Some(value.clone())).label(name.clone());
                                Tree::item(tree, row_value.clone(), props, cx)
                                    .child(
                                        Tree::label(tree, row_value.clone(), cx)
                                            .with(|el, state| row(el, state, ringed))
                                            .child(spacer())
                                            .child(name.clone()),
                                    )
                                    .into_any_element()
                            })
                            .collect()
                    })
                });
                part.child(label).children(group).into_any_element()
            })
            .collect()
    }

    // Pages, with what moving them needs: a row picks up with Space or a
    // drag, and the reorder takes every key while one is up.
    fn page_rows(&self, items: &[TreeItemData], focused: Option<&SharedString>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let tree = self.pages.clone();
        items
            .iter()
            .map(|item| {
                let value = item.value.clone();
                let ringed = focused == Some(&value);
                let moving = self.reorder.moving(self, &value);
                let example = cx.entity().downgrade();
                let keyed = value.clone();
                let own = tree.clone();
                let part = Tree::item(&tree, value.clone(), TreeItemProps::default(), cx).capture_key_down(
                    move |event, window, cx| {
                        if own.read(cx).focused_value(window).as_ref() != Some(&keyed) {
                            return;
                        }
                        example
                            .update(cx, |example, cx| {
                                let reorder = example.reorder.clone();
                                let up = reorder.draft(example).is_some();
                                let pressed = Key::of(&event.keystroke);
                                if !up && pressed == Key::Function(2) {
                                    let edit = example.edit.clone();
                                    if edit.keydown(example, event, keyed.clone(), window, cx) {
                                        example.begin_rename(window, cx);
                                        cx.stop_propagation();
                                    }
                                    return;
                                }
                                if !up && pressed != Key::Space {
                                    return;
                                }
                                // Tab puts the row back and moves on.
                                if reorder.keydown(example, event, &keyed, window, cx) && pressed != Key::Tab {
                                    cx.stop_propagation();
                                }
                            })
                            .ok();
                    },
                );
                self.reorder
                    .bind(value.clone(), TreeReorderItem::new().text(item.label.clone()), cx);
                let example = cx.entity().downgrade();
                let pressed = value.clone();
                let clicked = value.clone();
                let renamed = cx.entity().downgrade();
                let label = Tree::label(&tree, value.clone(), cx)
                    .with(|el, state| row(el, state, ringed))
                    .when(moving, |el| el.opacity(0.4))
                    .map_el(|el| match item.children {
                        Some(_) => el.child(indicator(&tree, &value, cx)),
                        None => el.child(spacer()),
                    })
                    .on_click(move |event, window, cx| {
                        if event.click_count() != 2 {
                            return;
                        }
                        let started = renamed
                            .update(cx, |example, cx| {
                                let edit = example.edit.clone();
                                let started = edit.dblclick(example, Some(InputEvent::from(event)), clicked.clone(), window, cx);
                                if started {
                                    example.begin_rename(window, cx);
                                }
                                started
                            })
                            .unwrap_or(false);
                        // The item's press would take focus from the field.
                        if started {
                            cx.stop_propagation();
                        }
                    });
                let renaming = self.edit.editing().as_ref() == Some(&value);
                let label = match self.rename.as_ref().filter(|_| renaming) {
                    Some((input, _)) => {
                        let typed = cx.entity().downgrade();
                        label.child(
                            TextField::root(input, cx)
                                .aria_label("Name")
                                .flex_1()
                                .px_1()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(0x2563eb))
                                .bg(rgb(0xffffff))
                                .on_key_down(move |event, window, cx| {
                                    typed
                                        .update(cx, |example, cx| {
                                            let edit = example.edit.clone();
                                            edit.input_keydown(example, event, window, cx);
                                        })
                                        .ok();
                                })
                                // A click in the field isn't a press on the item.
                                .on_click(|_, _, cx| cx.stop_propagation()),
                        )
                    }
                    None => label.child(item.label.clone()),
                };
                let label = on_pointer_down(label, move |event, window, cx| {
                    example
                        .update(cx, |example, cx| {
                            if example.edit.editing().as_ref() == Some(&pressed) {
                                return;
                            }
                            let reorder = example.reorder.clone();
                            reorder.pointer_down(example, event, &pressed, window, cx);
                        })
                        .ok();
                });
                let label = self.reorder.measure(value.clone(), label);
                let group = match &item.children {
                    Some(children) => Tree::group(&tree, value.clone(), TreeGroupProps::default(), cx).map(|group| {
                        let children = self.page_rows(children, focused, cx);
                        group
                            .with(|el, state| {
                                el.flex()
                                    .flex_col()
                                    .when_some(state.panel.height, |el, height| el.h(height).overflow_hidden())
                            })
                            .children(children)
                            .into_any_element()
                    }),
                    None => None,
                };
                part.child(label).children(group).into_any_element()
            })
            .collect()
    }
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.edit.editing().is_none() {
            self.rename = None;
        }
        // A move shows the rows where they would drop.
        let rows = self.reorder.rows(self);
        if rows != self.shown_rows {
            let items: Rc<[TreeItemData]> = pages_from_rows(&rows, &self.page_names).into();
            self.shown_rows = rows;
            self.pages
                .update(cx, |tree, cx| tree.set_options(pages_options(items), window, cx));
        }
        let ring = |tree: &Entity<Tree>, cx: &App| {
            focus_visible(window)
                .then(|| tree.read(cx).focused_value(window))
                .flatten()
        };
        let files_focus = ring(&self.files, cx);
        let mail_focus = ring(&self.mail, cx);
        let stack_focus = ring(&self.stack, cx);
        let database_focus = ring(&self.database, cx);
        let pages_focus = ring(&self.pages, cx);

        let files = &self.files;
        let files_items = files.read(cx).items().unwrap_or_default();
        let mail = &self.mail;
        let mail_items = mail.read(cx).items().unwrap_or_default();
        let stack = &self.stack;
        let stack_items = stack.read(cx).items().unwrap_or_default();
        let pages_items = self.pages.read(cx).items().unwrap_or_default();

        self.reorder.begin(cx);
        let pages_rows = self.page_rows(&pages_items, pages_focus.as_ref(), cx);
        let preview = self.reorder.preview().map(|preview| {
            let name = self.page_names.get(&preview.key).cloned().unwrap_or_default();
            preview.render(
                div()
                    .px_2()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xd1d5db))
                    .child(name),
            )
        });

        div()
            .size_full()
            .flex()
            .items_start()
            .gap_4()
            .p_4()
            .bg(rgb(0xffffff))
            .text_sm()
            .child(titled(
                "Files",
                Tree::root(files, cx)
                    .aria_label("Files")
                    .children(data_rows(files, &files_items, files_focus.as_ref(), cx)),
            ))
            .child(titled(
                "Mail",
                Tree::root(mail, cx)
                    .aria_label("Mail")
                    .children(data_rows(mail, &mail_items, mail_focus.as_ref(), cx)),
            ))
            .child(titled(
                "Stack",
                Tree::root(stack, cx)
                    .aria_label("Stack")
                    .children(data_rows(stack, &stack_items, stack_focus.as_ref(), cx)),
            ))
            .child(titled(
                "Database",
                Tree::root(&self.database, cx)
                    .aria_label("Database")
                    .children(self.database_rows(database_focus.as_ref(), cx)),
            ))
            .child(titled(
                "Pages",
                Tree::root(&self.pages, cx)
                    .aria_label("Pages")
                    .children(pages_rows),
            ))
            .child(self.reorder.listener())
            .children(preview)
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1080.), px(460.)), cx);
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
