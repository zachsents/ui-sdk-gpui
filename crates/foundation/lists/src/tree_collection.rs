//! Port of `foundation/lists/tree-collection.ts`: a tree's shape as plain
//! data: who each node's parent and children are, its level, its place among
//! its siblings, and which nodes show for a set of expanded branches. Trees,
//! nested menus and outlines read their structure from it, so no component
//! rebuilds parent tracking. Pure, so it runs anywhere.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::SharedString;

/// One node of a tree, as [`create_tree_collection`] reads it
/// (`TreeCollectionNode`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeCollectionNode {
    /// The node's key, unique in the tree.
    pub key: SharedString,
    /// The key of the node it's nested in, or `None` at the top level.
    pub parent: Option<SharedString>,
    /// Whether it has children, or can load them, even while none are known
    /// yet.
    pub branch: bool,
}

impl TreeCollectionNode {
    pub fn new(key: impl Into<SharedString>, parent: Option<SharedString>) -> Self {
        Self {
            key: key.into(),
            parent,
            branch: false,
        }
    }

    pub fn branch(mut self, branch: bool) -> Self {
        self.branch = branch;
        self
    }
}

struct Entry {
    parent: Option<SharedString>,
    children: Vec<SharedString>,
    branch: bool,
    index: usize,
}

/// A node's place among its siblings, both counted from 1, as
/// `aria-posinset` and `aria-setsize` count. Both are 0 for a key the tree
/// doesn't have.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreePosition {
    pub index: usize,
    pub size: usize,
}

/// A tree's structure (`createTreeCollection`'s return). Every lookup is a
/// map read or a walk up the node's ancestors; [`TreeCollection::visible`]
/// keeps its last answer while the expanded keys stay the same.
pub struct TreeCollection {
    entries: HashMap<SharedString, Entry>,
    roots: Vec<SharedString>,
    last: RefCell<Option<(Vec<SharedString>, Rc<[SharedString]>)>>,
}

impl std::fmt::Debug for TreeCollection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TreeCollection")
            .field("size", &self.entries.len())
            .field("roots", &self.roots)
            .finish()
    }
}

/// A tree's structure from its nodes, listed parents before children and
/// siblings in order (`createTreeCollection`). A node whose parent isn't
/// listed sits at the top level; a key listed twice keeps its first node.
///
/// ```ignore
/// let tree = create_tree_collection([
///     TreeCollectionNode::new("src", None),
///     TreeCollectionNode::new("src/index.ts", Some("src".into())),
///     TreeCollectionNode::new("README.md", None),
/// ]);
/// tree.level("src/index.ts"); // 2
/// tree.position("README.md"); // TreePosition { index: 2, size: 2 }
/// tree.visible(&[]); // ["src", "README.md"]
/// ```
pub fn create_tree_collection<N: Into<TreeCollectionNode>>(nodes: impl IntoIterator<Item = N>) -> TreeCollection {
    let mut entries: HashMap<SharedString, Entry> = HashMap::new();
    let mut order: Vec<SharedString> = Vec::new();
    for node in nodes {
        let node = node.into();
        if entries.contains_key(&node.key) {
            continue;
        }
        order.push(node.key.clone());
        entries.insert(
            node.key,
            Entry {
                parent: node.parent,
                children: Vec::new(),
                branch: node.branch,
                index: 0,
            },
        );
    }
    let mut roots: Vec<SharedString> = Vec::new();
    for key in order {
        let parent = entries
            .get(&key)
            .and_then(|entry| entry.parent.clone())
            .filter(|parent| entries.contains_key(parent));
        let index = match &parent {
            Some(parent) => {
                let siblings = &mut entries.get_mut(parent).expect("listed").children;
                siblings.push(key.clone());
                siblings.len()
            }
            None => {
                roots.push(key.clone());
                roots.len()
            }
        };
        if let Some(entry) = entries.get_mut(&key) {
            entry.parent = parent;
            entry.index = index;
        }
    }
    TreeCollection {
        entries,
        roots,
        last: RefCell::new(None),
    }
}

impl TreeCollection {
    /// How many nodes the tree has.
    pub fn size(&self) -> usize {
        self.entries.len()
    }

    /// Whether the tree has a node with this key.
    pub fn has(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// The key of the node `key` is nested in, or `None` at the top level.
    pub fn parent(&self, key: &str) -> Option<SharedString> {
        self.entries.get(key).and_then(|entry| entry.parent.clone())
    }

    /// The keys of a node's children in order, or of the top-level nodes for
    /// `None`.
    pub fn children(&self, key: Option<&str>) -> &[SharedString] {
        match key {
            None => &self.roots,
            Some(key) => self.entries.get(key).map(|entry| entry.children.as_slice()).unwrap_or(&[]),
        }
    }

    /// Whether a node has children, or was listed as a branch.
    pub fn is_branch(&self, key: &str) -> bool {
        self.entries
            .get(key)
            .is_some_and(|entry| entry.branch || !entry.children.is_empty())
    }

    /// A node's depth: 1 at the top level, as `aria-level` counts.
    pub fn level(&self, key: &str) -> usize {
        self.ancestors(key).len() + 1
    }

    /// A node's place among its siblings, both counted from 1, as
    /// `aria-posinset` and `aria-setsize` count.
    pub fn position(&self, key: &str) -> TreePosition {
        match self.entries.get(key) {
            None => TreePosition::default(),
            Some(entry) => TreePosition {
                index: entry.index,
                size: self.children(entry.parent.as_deref()).len(),
            },
        }
    }

    /// The keys of a node's ancestors, outermost first.
    pub fn ancestors(&self, key: &str) -> Vec<SharedString> {
        let mut found = Vec::new();
        let mut up = self.parent(key);
        while let Some(key) = up {
            up = self.parent(&key);
            found.push(key);
        }
        found.reverse();
        found
    }

    // Every node under `key` in order, or every node for `None`, walking
    // only into the branches `enter` allows.
    fn walk(&self, key: Option<&str>, enter: &dyn Fn(&str) -> bool, into: &mut Vec<SharedString>) {
        for child in self.children(key) {
            into.push(child.clone());
            if enter(child) {
                self.walk(Some(child), enter, into);
            }
        }
    }

    /// The keys of every node under `key`, in order.
    pub fn descendants(&self, key: &str) -> Vec<SharedString> {
        let mut into = Vec::new();
        self.walk(Some(key), &|_| true, &mut into);
        into
    }

    /// Every key in order, parents before their children.
    pub fn keys(&self) -> Vec<SharedString> {
        let mut into = Vec::new();
        self.walk(None, &|_| true, &mut into);
        into
    }

    /// The keys that show when only the branches in `expanded` are open: the
    /// top-level nodes, and the children of every expanded branch whose
    /// ancestors are expanded too, in order. Kept while `expanded` holds the
    /// same keys.
    pub fn visible(&self, expanded: &[SharedString]) -> Rc<[SharedString]> {
        if let Some((last, visible)) = self.last.borrow().as_ref()
            && last.as_slice() == expanded
        {
            return visible.clone();
        }
        let open: HashSet<&str> = expanded.iter().map(|key| key.as_ref()).collect();
        let mut into = Vec::new();
        self.walk(None, &|key| open.contains(key), &mut into);
        let visible: Rc<[SharedString]> = into.into();
        *self.last.borrow_mut() = Some((expanded.to_vec(), visible.clone()));
        visible
    }
}

/// A node of tree data: a value, the text it shows, and its children
/// (`TreeItemData`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeItemData {
    /// Identifies the item. Unique in the tree.
    pub value: SharedString,
    /// What it shows, and what typeahead matches.
    pub label: SharedString,
    /// Its children. `None` for a leaf. An empty list is a branch with no
    /// children, or one whose children load when it opens.
    pub children: Option<Vec<TreeItemData>>,
    /// Whether it can't be selected, checked or activated. It can still take
    /// focus.
    pub disabled: bool,
    /// Makes the item a link to this address.
    pub href: Option<SharedString>,
}

impl TreeItemData {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            ..Self::default()
        }
    }

    pub fn children(mut self, children: impl IntoIterator<Item = TreeItemData>) -> Self {
        self.children = Some(children.into_iter().collect());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn href(mut self, href: impl Into<SharedString>) -> Self {
        self.href = Some(href.into());
        self
    }
}

/// Data shaped like [`TreeItemData`], as far as a tree reads it: a value and
/// children. The original takes any object of that shape, like a
/// waterfall's spans, wherever it takes tree data; here such data
/// implements this.
pub trait TreeData: Sized {
    /// Identifies the item. Unique in the tree.
    fn value(&self) -> &SharedString;
    /// Its children: `None` for a leaf, and an empty list for a branch with
    /// none, or one whose children load when it opens.
    fn children(&self) -> Option<&[Self]>;
}

impl TreeData for TreeItemData {
    fn value(&self) -> &SharedString {
        &self.value
    }

    fn children(&self) -> Option<&[Self]> {
        self.children.as_deref()
    }
}

/// A node of tree data as a collection node, with the item it came from.
#[derive(Debug, PartialEq, Eq)]
pub struct TreeDataNode<'a, I = TreeItemData> {
    pub key: SharedString,
    pub parent: Option<SharedString>,
    pub branch: bool,
    /// The item from the data, the same one passed in.
    pub item: &'a I,
}

impl<I> Clone for TreeDataNode<'_, I> {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            parent: self.parent.clone(),
            branch: self.branch,
            item: self.item,
        }
    }
}

impl<I> From<TreeDataNode<'_, I>> for TreeCollectionNode {
    fn from(node: TreeDataNode<'_, I>) -> Self {
        Self {
            key: node.key,
            parent: node.parent,
            branch: node.branch,
        }
    }
}

impl<I> From<&TreeDataNode<'_, I>> for TreeCollectionNode {
    fn from(node: &TreeDataNode<'_, I>) -> Self {
        Self {
            key: node.key.clone(),
            parent: node.parent.clone(),
            branch: node.branch,
        }
    }
}

/// Lists tree data as collection nodes, parents before children
/// (`treeDataNodes`). An item with children, even none, is a branch.
pub fn tree_data_nodes<I: TreeData>(items: &[I]) -> Vec<TreeDataNode<'_, I>> {
    fn add<'a, I: TreeData>(list: &'a [I], parent: Option<&SharedString>, nodes: &mut Vec<TreeDataNode<'a, I>>) {
        for item in list {
            let children = item.children();
            nodes.push(TreeDataNode {
                key: item.value().clone(),
                parent: parent.cloned(),
                branch: children.is_some(),
                item,
            });
            if let Some(children) = children {
                add(children, Some(item.value()), nodes);
            }
        }
    }
    let mut nodes = Vec::new();
    add(items, None, &mut nodes);
    nodes
}

// A node of the tree `tree_from_paths` builds, before it's data.
struct PathNode {
    value: SharedString,
    label: SharedString,
    // A folder's children, by index.
    children: Option<Vec<usize>>,
}

/// Tree data from a list of file paths, like `["src/index.ts", "README.md"]`
/// (`treeFromPaths`). Each folder becomes a branch whose value is its path,
/// and each file a leaf. A path ending in `/` is an empty folder. Folders
/// come first, then files, each in the order the list gives.
pub fn tree_from_paths<P: AsRef<str>>(paths: &[P]) -> Vec<TreeItemData> {
    let mut nodes: Vec<PathNode> = Vec::new();
    let mut top: Vec<usize> = Vec::new();
    let mut folders: HashMap<String, usize> = HashMap::new();
    for path in paths {
        let mut segments: Vec<&str> = path.as_ref().split('/').collect();
        let is_folder = segments.last() == Some(&"");
        if is_folder {
            segments.pop();
        }
        // `None` is the top level, else a folder's node.
        let mut into: Option<usize> = None;
        let mut prefix = String::new();
        let count = segments.len();
        for (index, segment) in segments.into_iter().enumerate() {
            prefix = if prefix.is_empty() {
                segment.to_string()
            } else {
                format!("{prefix}/{segment}")
            };
            let siblings_has = |nodes: &Vec<PathNode>, top: &Vec<usize>, into: Option<usize>, value: &str| {
                let list = match into {
                    None => top.as_slice(),
                    Some(folder) => nodes[folder].children.as_deref().unwrap_or_default(),
                };
                list.iter().any(|&child| nodes[child].value == value)
            };
            if index < count - 1 || is_folder {
                let folder = match folders.get(&prefix) {
                    Some(&folder) => folder,
                    None => {
                        let folder = nodes.len();
                        nodes.push(PathNode {
                            value: prefix.clone().into(),
                            label: segment.to_string().into(),
                            children: Some(Vec::new()),
                        });
                        folders.insert(prefix.clone(), folder);
                        match into {
                            None => top.push(folder),
                            Some(parent) => {
                                if let Some(children) = nodes[parent].children.as_mut() {
                                    children.push(folder);
                                }
                            }
                        }
                        folder
                    }
                };
                into = Some(folder);
            } else if !siblings_has(&nodes, &top, into, &prefix) {
                let leaf = nodes.len();
                nodes.push(PathNode {
                    value: prefix.clone().into(),
                    label: segment.to_string().into(),
                    children: None,
                });
                match into {
                    None => top.push(leaf),
                    Some(parent) => {
                        if let Some(children) = nodes[parent].children.as_mut() {
                            children.push(leaf);
                        }
                    }
                }
            }
        }
    }
    fn folders_first(nodes: &[PathNode], list: &[usize]) -> Vec<TreeItemData> {
        let folders = list.iter().filter(|&&index| nodes[index].children.is_some());
        let files = list.iter().filter(|&&index| nodes[index].children.is_none());
        folders
            .chain(files)
            .map(|&index| {
                let node = &nodes[index];
                TreeItemData {
                    value: node.value.clone(),
                    label: node.label.clone(),
                    children: node.children.as_ref().map(|children| folders_first(nodes, children)),
                    disabled: false,
                    href: None,
                }
            })
            .collect()
    }
    folders_first(&nodes, &top)
}

/// One row of tree data that shows, with what a flat rendering needs to
/// place it (`TreeRow`).
#[derive(Debug, PartialEq, Eq)]
pub struct TreeRow<'a, I = TreeItemData> {
    /// The item from the data this row shows, the same one passed in,
    /// children included.
    pub item: &'a I,
    /// The value of the item it's nested in, or `None` at the top level.
    pub parent: Option<&'a SharedString>,
    /// Its depth, 1 at the top level.
    pub level: usize,
    /// Its place among the rows that show, from 0, for a virtualized list.
    pub index: usize,
}

impl<I> Clone for TreeRow<'_, I> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<I> Copy for TreeRow<'_, I> {}

/// The rows of tree data that show when the branches in `expanded` are
/// open, in order, for a flat rendering such as a virtualized list
/// (`visibleTreeRows`).
pub fn visible_tree_rows<'a, I: TreeData>(items: &'a [I], expanded: &[SharedString]) -> Vec<TreeRow<'a, I>> {
    fn add<'a, I: TreeData>(
        list: &'a [I],
        parent: Option<&'a SharedString>,
        level: usize,
        open: &HashSet<&str>,
        rows: &mut Vec<TreeRow<'a, I>>,
    ) {
        for item in list {
            let index = rows.len();
            rows.push(TreeRow {
                item,
                parent,
                level,
                index,
            });
            if let Some(children) = item.children()
                && open.contains(item.value().as_ref())
            {
                add(children, Some(item.value()), level + 1, open, rows);
            }
        }
    }
    let open: HashSet<&str> = expanded.iter().map(|key| key.as_ref()).collect();
    let mut rows = Vec::new();
    add(items, None, 1, &open, &mut rows);
    rows
}
