//! Port of `foundation/lists/tree-checked.ts`: tri-state checkboxes over a
//! tree: checking a branch checks everything in it, a branch shows mixed
//! while only some of it is checked, and the checked value reports the nodes
//! a strategy asks for. Pure, over a [`TreeCollection`].

use std::collections::{HashMap, HashSet};

use gpui::SharedString;
use ui_sdk_kernel::Checked;

use crate::tree_collection::TreeCollection;

/// Which nodes a tree's checked value lists, and whether checks cascade
/// (`TreeCheckStrategy`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TreeCheckStrategy {
    /// The checked leaves. A branch is checked when all its leaves are.
    #[default]
    Leaf,
    /// The outermost fully checked nodes, so a checked folder stands for
    /// everything in it.
    Trunk,
    /// Every fully checked node, branches and leaves.
    Branch,
    /// Each node on its own, with no cascading and no mixed state.
    Independent,
}

/// A node's checkbox: checked, unchecked, or indeterminate while only some
/// of a branch is checked (`TreeCheckedState`). Indeterminate renders as
/// `aria-checked="mixed"`.
pub type TreeCheckedState = Checked;

// A node with no known children counts as a leaf, so a branch whose children
// haven't loaded yet checks like one, and passes its check on once they load.
fn is_leaf(tree: &TreeCollection, key: &str) -> bool {
    tree.children(Some(key)).is_empty()
}

// The checked leaves a value stands for. A branch in the value stands for
// every leaf under it, whatever the strategy, so a value written for another
// strategy, or one checked before the branch's children loaded, still reads
// right.
fn checked_leaves(tree: &TreeCollection, value: &[SharedString]) -> HashSet<SharedString> {
    let mut leaves = HashSet::new();
    for key in value {
        if !tree.has(key) {
            continue;
        }
        if is_leaf(tree, key) {
            leaves.insert(key.clone());
        } else {
            for below in tree.descendants(key) {
                if is_leaf(tree, &below) {
                    leaves.insert(below);
                }
            }
        }
    }
    leaves
}

/// Every node's checkbox for a checked value, worked out in one pass over
/// the tree, so reading a node is a map lookup however big its branch is
/// ([`tree_checked_states`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeCheckedStates {
    states: HashMap<SharedString, TreeCheckedState>,
}

impl TreeCheckedStates {
    /// A node's checkbox.
    pub fn get(&self, key: &str) -> TreeCheckedState {
        self.states.get(key).copied().unwrap_or(Checked::False)
    }
}

/// Every node's checkbox for a checked value (`treeCheckedStates`). Read
/// each node with [`TreeCheckedStates::get`].
pub fn tree_checked_states(tree: &TreeCollection, value: &[SharedString], strategy: TreeCheckStrategy) -> TreeCheckedStates {
    let mut states = HashMap::new();
    if strategy == TreeCheckStrategy::Independent {
        for key in value {
            states.insert(key.clone(), Checked::True);
        }
        return TreeCheckedStates { states };
    }
    let leaves = checked_leaves(tree, value);
    // Post order: each branch counts its leaves and checked leaves from its
    // children.
    fn count(
        tree: &TreeCollection,
        key: &SharedString,
        leaves: &HashSet<SharedString>,
        states: &mut HashMap<SharedString, TreeCheckedState>,
    ) -> (usize, usize) {
        if is_leaf(tree, key) {
            let checked = leaves.contains(key);
            if checked {
                states.insert(key.clone(), Checked::True);
            }
            return (1, usize::from(checked));
        }
        let mut total = 0;
        let mut checked = 0;
        for child in tree.children(Some(key)) {
            let (child_total, child_checked) = count(tree, child, leaves, states);
            total += child_total;
            checked += child_checked;
        }
        if checked > 0 {
            states.insert(
                key.clone(),
                if checked == total {
                    Checked::True
                } else {
                    Checked::Indeterminate
                },
            );
        }
        (total, checked)
    }
    for root in tree.children(None) {
        count(tree, root, &leaves, &mut states);
    }
    TreeCheckedStates { states }
}

// The value a strategy reports for a set of checked leaves, in tree order.
fn report(tree: &TreeCollection, leaves: &HashSet<SharedString>, strategy: TreeCheckStrategy) -> Vec<SharedString> {
    let checked: Vec<SharedString> = leaves.iter().cloned().collect();
    let state = tree_checked_states(tree, &checked, TreeCheckStrategy::Leaf);
    let keys = tree.keys();
    match strategy {
        TreeCheckStrategy::Leaf => keys.into_iter().filter(|key| leaves.contains(key)).collect(),
        TreeCheckStrategy::Branch => keys
            .into_iter()
            .filter(|key| state.get(key) == Checked::True)
            .collect(),
        // Trunk: a fully checked node whose parent isn't fully checked.
        _ => keys
            .into_iter()
            .filter(|key| {
                if state.get(key) != Checked::True {
                    return false;
                }
                tree.parent(key)
                    .is_none_or(|parent| state.get(&parent) != Checked::True)
            })
            .collect(),
    }
}

/// What [`toggle_tree_checked`] takes: the strategy, and which nodes may
/// change, like all but the disabled ones, which may read the component
/// asking.
#[derive(Clone, Copy, Default)]
pub struct ToggleTreeCheckedOptions<'a> {
    pub strategy: TreeCheckStrategy,
    pub checkable: Option<&'a dyn Fn(&str) -> bool>,
}

impl<'a> ToggleTreeCheckedOptions<'a> {
    pub fn strategy(mut self, strategy: TreeCheckStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    pub fn checkable(mut self, checkable: &'a dyn Fn(&str) -> bool) -> Self {
        self.checkable = Some(checkable);
        self
    }
}

/// The checked value after a node's checkbox is pressed
/// (`toggleTreeChecked`). A branch whose checkable leaves are all checked
/// unchecks them, and any other checks them; nodes `checkable` refuses, like
/// disabled ones, keep their state.
pub fn toggle_tree_checked(
    tree: &TreeCollection,
    value: &[SharedString],
    key: &str,
    options: &ToggleTreeCheckedOptions,
) -> Vec<SharedString> {
    let checkable = |node: &str| options.checkable.is_none_or(|checkable| checkable(node));
    if !checkable(key) {
        return value.to_vec();
    }
    if options.strategy == TreeCheckStrategy::Independent {
        return if value.iter().any(|checked| checked == key) {
            value.iter().filter(|checked| *checked != key).cloned().collect()
        } else {
            value
                .iter()
                .cloned()
                .chain(std::iter::once(SharedString::from(key.to_string())))
                .collect()
        };
    }
    let mut leaves = checked_leaves(tree, value);
    let below: Vec<SharedString> = if is_leaf(tree, key) {
        vec![SharedString::from(key.to_string())]
    } else {
        tree.descendants(key)
            .into_iter()
            .filter(|node| is_leaf(tree, node))
            .collect()
    };
    let depth = tree.ancestors(key).len() + 1;
    // A refused node keeps its state and shields what's under it.
    let affected: Vec<SharedString> = below
        .into_iter()
        .filter(|leaf| {
            checkable(leaf)
                && tree
                    .ancestors(leaf)
                    .iter()
                    .skip(depth)
                    .all(|node| checkable(node))
        })
        .collect();
    // Checks unless every leaf it can change is checked, so a branch with a
    // refused leaf still unchecks.
    let check = !affected.iter().all(|leaf| leaves.contains(leaf));
    for leaf in affected {
        if check {
            leaves.insert(leaf);
        } else {
            leaves.remove(&leaf);
        }
    }
    report(tree, &leaves, options.strategy)
}
