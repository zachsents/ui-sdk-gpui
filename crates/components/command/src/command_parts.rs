//! Port of `components/command/command-parts.ts`: Command's parts and what
//! they carry.
//!
//! The app renders the parts it wants, in page order: the `root` around the
//! search field (`input`), the empty and status messages, and the `list`
//! holding commands (`item`), with `item-text`, `item-icon` and `shortcut`
//! inside them, `group`s with their `group-label`, `separator`s, and
//! `page`s. A page part is `None` while it isn't the current page, so the
//! commands inside it don't render, as the original's page leaves the page.

use std::panic::Location;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, ElementId, Entity, Focusable, GlobalElementId, InspectorElementId, KeyDownEvent,
    LayoutId, Pixels, SharedString, Style, WeakEntity, Window,
};
use ui_sdk_focus::hold_focus;
use ui_sdk_kernel::dev;
use ui_sdk_kernel::part::FocusMode;
use ui_sdk_kernel::roles::{HandledKey, Need, Role};
use ui_sdk_kernel::{Aria, AutoComplete, InputEvent, Part, Reason};
use ui_sdk_lists::{
    AsyncOptionsStatus, AutocompleteCoreItem, AutocompleteOptionState, ItemIconProps, ItemIconState,
    ListEmptyState, ListItemTextProps, ListItemTextState, ListStatusState, OptionGroupState, OptionPressFn,
    OptionSeparatorState, check_icon_item, empty_part, field_part, group_label_part, has_icon, item_icon_part,
    item_text_part, list_name, option_group_part, option_icon, option_part, option_separator_part, status_part,
    warn_if_unnamed,
};
use ui_sdk_native::TextField;
use ui_sdk_platform::{aria_keyshortcuts, is_apple};
use ui_sdk_shortcuts::{
    RegisteredShortcut, ShortcutLayer, ShortcutOptions, ShortcutPart, ShortcutProps, bind_part_shortcut,
    register_shortcut, shortcut_slot,
};

use crate::command::{
    Command, CommandInputState, CommandItemProps, CommandItemState, CommandListState, CommandState,
};
use crate::command_data::{CommandData, command_shortcuts, find_command};

// The search field's keys: the core's steps, and the command's own.
const INPUT_KEYS: &[HandledKey] = &[
    "ArrowDown",
    "ArrowUp",
    "PageDown",
    "PageUp",
    "ctrl+n",
    "ctrl+p",
    "Enter",
    "Escape",
    "Backspace",
];

impl Command {
    // What a command shows beside the option it is: whether it's running or
    // failed (`itemState`).
    fn item_state(&self, key: &SharedString, option: AutocompleteOptionState) -> CommandItemState {
        CommandItemState {
            highlighted: option.highlighted,
            disabled: option.disabled,
            pending: self.pending.contains(key),
            error: self
                .error
                .as_ref()
                .filter(|(failed, _)| failed == key)
                .map(|(_, reason)| reason.clone()),
        }
    }

    // The search field rendered: development builds check, once, that
    // something names it.
    pub(crate) fn input_rendered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.input_bound, true) {
            return;
        }
        warn_if_unnamed(
            |this: &Self| &this.instance,
            "input",
            None,
            "The search field has no accessible name. Add an aria_label or a label.",
            |_this: &Self, _cx| false,
            window,
            cx,
        );
    }

    // Where shortcuts belong: the search field, inside the root, so a
    // command in a dialog runs its shortcuts while the dialog is open.
    fn shortcut_layer(&self, cx: &App) -> ShortcutLayer {
        ShortcutLayer::from(self.field.read(cx).focus_handle(cx))
    }

    /// The `root` part: the element around the field and the list. Data
    /// commands' shortcuts run from anywhere while it's in the page,
    /// whatever page or filter hides them.
    pub fn root(this: &Entity<Self>, cx: &App) -> Part<CommandState> {
        let command = this.read(cx);
        let state = CommandState {
            page: command.current_page(),
            loading: command.status == AsyncOptionsStatus::Loading,
        };
        let shortcuts = DataShortcuts {
            items: command.items.clone(),
            layer: command.shortcut_layer(cx),
            entity: this.downgrade(),
        };
        Part::new(&command.instance, "root", None, state).child(shortcuts)
    }

    /// The `input` part: the search field, a combobox that keeps focus while
    /// arrow keys move the highlight. Name it with `aria_label`.
    pub fn input(this: &Entity<Self>, cx: &App) -> Part<CommandInputState> {
        let command = this.read(cx);
        let state = CommandInputState {
            page: command.current_page(),
        };
        let focus = command.field.read(cx).focus_handle(cx);
        let (el, _) = TextField::root(&command.field, cx).into_element_parts();
        let part = Part::new(&command.instance, "input", None, state).map_el(move |_| el);
        let part = field_part(
            this,
            &command.instance,
            part,
            true,
            command.core.active_descendant(command),
            INPUT_KEYS,
            |command: &mut Command, event: &KeyDownEvent, window, cx| command.on_input_key_down(event, window, cx),
        )
        .focus_mode(FocusMode::Native, Some(&focus), false)
        .aria(
            Aria::new()
                .autocomplete(AutoComplete::List)
                .controls(command.instance.element_id("list", None)),
        );
        if !dev::DEV {
            return part;
        }
        let bound = this.downgrade();
        part.keep_name().on_props(move |window, cx| {
            bound.update(cx, |command, cx| command.input_rendered(window, cx)).ok();
        })
    }

    /// The `list` part: the listbox holding the commands, always shown. The
    /// search field takes its keys, and a press on it leaves focus there.
    /// It tracks the command's [`scroll_handle`](Command::scroll_handle):
    /// give it a height limit and `overflow_y_scroll()` for long lists, and
    /// the highlighted command scrolls into view. Render it before its
    /// commands.
    pub fn list(this: &Entity<Self>, cx: &App) -> Part<CommandListState> {
        let command = this.read(cx);
        command.core.listbox.begin(cx);
        // The commands render again after it, with what they render with now.
        command.item_props.borrow_mut().clear();
        let items: CommandData = command
            .core
            .listbox
            .visible_data(command)
            .map(|visible| visible.list)
            .unwrap_or_default();
        let loading = command.status == AsyncOptionsStatus::Loading;
        let state = CommandListState {
            status: command.status,
            items,
        };
        let part = Part::new(&command.instance, "list", None, state)
            .aria_role(Role::Listbox)
            .unneeded(
                Need::Keys,
                "The search field handles them, and keeps focus while they move the highlight.",
            )
            .aria(list_name(false, None, &command.instance).busy(loading))
            .on_any_mouse_down(hold_focus)
            .track_scroll(&command.scroll);
        // Commands render in the same frame as the list. Once it's drawn, an
        // empty list is really empty.
        let part = command.settled.track(this.entity_id(), part);
        command.core.load_more.bind(part, command.scroll.clone(), this.downgrade())
    }

    /// The `item` part: a command. Pressing it runs it and leaves focus in
    /// the search field; the pointer highlights it. The search hides it
    /// while it doesn't match. With a `shortcut`, its keys run it while it's
    /// in the page, unless it comes from the data, whose shortcuts the root
    /// runs.
    pub fn item(this: &Entity<Self>, props: CommandItemProps, cx: &App) -> Part<CommandItemState> {
        let command = this.read(cx);
        let key = props.value.clone();
        command
            .item_props
            .borrow_mut()
            .insert(key.clone(), props.clone());
        let item = AutocompleteCoreItem {
            value: key.clone(),
            label: props.label.clone(),
            text: None,
            keywords: props.keywords.clone(),
            disabled: props.disabled,
            index: props.index,
            group: props.group.clone(),
            link: props.link,
        };
        let press: OptionPressFn<Self> = Rc::new(|command: &mut Self, key: &SharedString, event, window, cx| {
            command.run_item(key.clone(), Some(event), Reason::ItemPress, false, window, cx)
        });
        let mut part = option_part(this, &command.instance, "item", &command.core, command, &key, &item, press, cx)
            .map_state(|option| command.item_state(&key, option));
        let pending = part.state().pending;
        part = part.aria(Aria::new().disabled(pending).busy(pending));
        if let Some(label) = &props.label {
            part = part.aria_label(label.clone());
        }
        if let Some(shortcut) = props.shortcut.as_ref().filter(|shortcut| !shortcut.is_empty()) {
            part = part.aria_keyshortcuts(SharedString::from(aria_keyshortcuts(shortcut, is_apple(cx))));
        }
        // A command that shows only an icon names nothing.
        if dev::DEV && has_icon(&command.instance, &key) {
            let entity = this.downgrade();
            let key = key.clone();
            part = part.keep_name().on_props(move |window, cx| {
                if let Some(command) = entity.upgrade() {
                    check_icon_item(&command.read(cx).instance, "item", Some(&key), option_icon("command"), window);
                }
            });
        }
        // A composed command's shortcut. The root already runs the shortcuts
        // of commands from the data.
        let from_data = props.data
            || find_command(command.items.as_ref(), &key).is_some_and(|data| data.extra.shortcut.is_some());
        let keys = if from_data { None } else { props.shortcut.clone() };
        let shortcut = this.downgrade();
        let run_key = key.clone();
        bind_part_shortcut(
            part,
            keys,
            move |event, window, cx| {
                shortcut
                    .update(cx, |command, cx| {
                        command.run_item(
                            run_key.clone(),
                            Some(InputEvent::from(event)),
                            Reason::Shortcut,
                            true,
                            window,
                            cx,
                        )
                    })
                    .ok();
            },
            ShortcutOptions::default().layer(command.shortcut_layer(cx)),
        )
    }

    /// The `item-text` part: a command's text, split where the search
    /// matched, so the matches can be marked. Render `state.segments`. The
    /// text is `props.text`, or the command's label.
    pub fn item_text(this: &Entity<Self>, item: &str, props: ListItemTextProps, cx: &App) -> Part<ListItemTextState> {
        let command = this.read(cx);
        let key = SharedString::from(item.to_string());
        let text = if props.text.is_empty() {
            command
                .item_props
                .borrow()
                .get(item)
                .and_then(|props| props.label.clone())
                .unwrap_or_else(|| key.clone())
        } else {
            props.text
        };
        let query = command.core.filter_query(command);
        let state = command
            .texts
            .state(&key, &text, &query, command.core.match_mode(command));
        item_text_part(&command.instance, item, state)
    }

    /// The `item-icon` part: an icon inside a command, by the command's
    /// value. Decorative.
    pub fn item_icon(this: &Entity<Self>, item: &str, props: &ItemIconProps, cx: &App) -> Part<ItemIconState> {
        let command = this.read(cx);
        item_icon_part(&command.instance, Some(item), props, cx)
    }

    /// The `group` part: commands under a label, by the group's key, which
    /// its commands name in their `group` prop. Hidden while none of its
    /// commands match.
    pub fn group(this: &Entity<Self>, key: &str, cx: &App) -> Part<OptionGroupState> {
        let command = this.read(cx);
        option_group_part(&command.instance, key, &command.core.listbox, command)
    }

    /// The `group-label` part: what names its group.
    pub fn group_label(this: &Entity<Self>, group: &str, cx: &App) -> Part<()> {
        let command = this.read(cx);
        group_label_part(&command.instance, group)
    }

    /// The `separator` part: a line between commands or groups, left out
    /// for assistive technology. Composed separators hide while the search
    /// filters.
    pub fn separator(this: &Entity<Self>, key: Option<&str>, cx: &App) -> Part<OptionSeparatorState> {
        let command = this.read(cx);
        let filtering = !command.core.filter_query(command).is_empty() && command.items.is_none();
        option_separator_part(&command.instance, key, filtering)
    }

    /// The `empty` part: a polite status shown while no command matches.
    /// Render its message while `state.empty`.
    pub fn empty(this: &Entity<Self>, cx: &App) -> Part<ListEmptyState> {
        let command = this.read(cx);
        let state = ListEmptyState {
            loading: command.status == AsyncOptionsStatus::Loading,
            empty: command.core.is_empty(command),
            query: command.query.get().clone(),
        };
        empty_part(&command.instance, state)
    }

    /// The `status` part: a polite live region for loading and result
    /// counts. Its content is yours; with it in the page, the command says
    /// no counts itself.
    pub fn status(this: &Entity<Self>, cx: &App) -> Part<ListStatusState> {
        let command = this.read(cx);
        let state = ListStatusState {
            status: command.status,
            loading: command.status == AsyncOptionsStatus::Loading,
            count: command.core.item_count(command),
        };
        status_part(&command.instance, state)
    }

    /// The `page` part: a page of commands, by the value a command's `page`
    /// opens, or `None` for the first page. It's `None` while it isn't the
    /// page on top of the stack, so the commands inside don't render.
    pub fn page(this: &Entity<Self>, value: Option<&str>, cx: &App) -> Option<Part<()>> {
        let command = this.read(cx);
        if command.current_page().as_deref() != value {
            return None;
        }
        Some(Part::new(&command.instance, "page", value, ()).aria_role(Role::None))
    }

    /// The `shortcut` part: the keys of a command's shortcut, for people to
    /// see, by the command's value: its `shortcut`, or `props.shortcut`.
    /// Screen readers hear the command's `aria-keyshortcuts` instead.
    pub fn shortcut(this: &Entity<Self>, item: Option<&str>, props: ShortcutProps, cx: &App) -> ShortcutPart {
        let command = this.read(cx);
        let inherited = item.and_then(|key| {
            command
                .item_props
                .borrow()
                .get(key)
                .and_then(|props| props.shortcut.clone())
                .or_else(|| find_command(command.items.as_ref(), key).and_then(|data| data.extra.shortcut))
        });
        shortcut_slot(&command.instance, item, props, inherited, is_apple(cx))
    }
}

// What the root puts in its element for the data commands' shortcuts: they
// register while it renders, again when the data changes, and unregister once
// a frame doesn't render the root.
struct DataShortcuts {
    items: Option<CommandData>,
    layer: ShortcutLayer,
    entity: WeakEntity<Command>,
}

struct BoundShortcuts {
    items: Option<CommandData>,
    _registered: Vec<RegisteredShortcut>,
}

fn same_items(a: &Option<CommandData>, b: &Option<CommandData>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.ptr_eq(b),
        (None, None) => true,
        _ => false,
    }
}

impl Element for DataShortcuts {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("ui-sdk-command-shortcuts".into()))
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        if let Some(id) = id {
            let items = self.items.clone();
            let layer = self.layer.clone();
            let entity = self.entity.clone();
            window.with_element_state::<BoundShortcuts, _>(id, |bound, _window| {
                let bound = match bound {
                    Some(bound) if same_items(&bound.items, &items) => bound,
                    stale => {
                        // The data changed: the old shortcuts go first.
                        drop(stale);
                        let registered = command_shortcuts(items.as_ref())
                            .into_iter()
                            .map(|found| {
                                let entity = entity.clone();
                                let value = found.value.clone();
                                register_shortcut(
                                    found.shortcut,
                                    move |event: &KeyDownEvent, window: &mut Window, cx: &mut App| {
                                        entity
                                            .update(cx, |command, cx| {
                                                command.run_item(
                                                    value.clone(),
                                                    Some(InputEvent::from(event)),
                                                    Reason::Shortcut,
                                                    true,
                                                    window,
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    },
                                    ShortcutOptions::default().layer(layer.clone()),
                                    cx,
                                )
                            })
                            .collect();
                        BoundShortcuts {
                            items,
                            _registered: registered,
                        }
                    }
                };
                ((), bound)
            });
        }
        let style = Style {
            position: gpui::Position::Absolute,
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}
