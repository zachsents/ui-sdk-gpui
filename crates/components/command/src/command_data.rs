//! Port of `components/command/command-data.ts`: commands as data, finding a
//! command anywhere in the tree, the commands of the current page, and every
//! shortcut to bind.

use gpui::SharedString;
use ui_sdk_lists::{ListData, ListGroup, ListItem, ListNode};

/// What a command as data has beside a list item's value, label, keywords
/// and `disabled` (`CommandDataItem`'s own fields).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandDataFields {
    /// A keyboard shortcut that runs the command, like `"mod+p"`, while the
    /// list is in the page, whatever page it's on. Shows `⌘P` on Apple
    /// platforms and `Ctrl+P` elsewhere.
    pub shortcut: Option<SharedString>,
    /// Commands on a page of their own. Choosing this command opens the
    /// page, and Backspace in the empty field goes back.
    pub items: Option<CommandData>,
}

/// A command as data.
pub type CommandDataItem = ListItem<SharedString, CommandDataFields>;

/// A labelled group of commands, as data.
pub type CommandDataGroup = ListGroup<SharedString, CommandDataFields>;

/// Commands as data: commands, plain strings, groups and separators. A plain
/// string is its own value and label.
pub type CommandData = ListData<SharedString, CommandDataFields>;

/// A command's own fields, as a command's builder methods.
pub trait CommandDataItemExt {
    /// A keyboard shortcut that runs the command, like `"mod+p"`.
    fn shortcut(self, shortcut: impl Into<SharedString>) -> Self;
    /// Commands on a page this command opens.
    fn items(self, items: CommandData) -> Self;
}

impl CommandDataItemExt for CommandDataItem {
    fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.extra.shortcut = Some(shortcut.into());
        self
    }

    fn items(mut self, items: CommandData) -> Self {
        self.extra.items = Some(items);
        self
    }
}

/// A command: its value and the label the search matches.
pub fn command(value: impl Into<SharedString>, label: impl Into<SharedString>) -> CommandDataItem {
    ListItem::new(value.into(), label)
}

// Every command at one level, inside groups too, without descending into pages.
fn commands_at(items: &CommandData) -> impl Iterator<Item = &CommandDataItem> {
    items.nodes().iter().flat_map(|node| {
        let commands: &[CommandDataItem] = match node {
            ListNode::Item(item) => std::slice::from_ref(item),
            ListNode::Group(group) => &group.items,
            ListNode::Separator(_) => &[],
        };
        commands.iter()
    })
}

/// The commands a stack of pages shows: each page is the `items` of the
/// command with that value on the page below. `None` when a page isn't in
/// the data, so composed pages can supply their own.
pub fn page_items(items: Option<&CommandData>, pages: &[SharedString]) -> Option<CommandData> {
    let mut level = items.cloned();
    for page in pages {
        let current = level?;
        let mut next = None;
        for command in commands_at(&current) {
            if command.value == *page {
                next = command.extra.items.clone();
            }
        }
        level = next;
    }
    level
}

/// The command with this value anywhere in the data, pages included.
pub fn find_command(items: Option<&CommandData>, value: &str) -> Option<CommandDataItem> {
    let items = items?;
    for command in commands_at(items) {
        if command.value.as_ref() == value {
            return Some(command.clone());
        }
        if let Some(nested) = find_command(command.extra.items.as_ref(), value) {
            return Some(nested);
        }
    }
    None
}

/// A command with a shortcut: its value and the keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandShortcut {
    pub value: SharedString,
    pub shortcut: SharedString,
}

/// Every enabled command with a shortcut, pages included.
pub fn command_shortcuts(items: Option<&CommandData>) -> Vec<CommandShortcut> {
    fn visit(level: &CommandData, found: &mut Vec<CommandShortcut>) {
        for command in commands_at(level) {
            if let Some(shortcut) = command.extra.shortcut.as_ref().filter(|shortcut| !shortcut.is_empty())
                && !command.disabled
            {
                found.push(CommandShortcut {
                    value: command.value.clone(),
                    shortcut: shortcut.clone(),
                });
            }
            if let Some(items) = &command.extra.items {
                visit(items, found);
            }
        }
    }
    let mut found = Vec::new();
    if let Some(items) = items {
        visit(items, &mut found);
    }
    found
}
