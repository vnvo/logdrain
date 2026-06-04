//! Prefix tree within a shard + per-leaf cluster bucket.

use std::sync::Arc;

use rustc_hash::FxBuildHasher;

use crate::ClusterId;

/// Leaf bucket: the set of cluster ids at a leaf, with a capacity. Bodies live in
/// the miner's `clusters_by_id` map and carry their own recency, so the leaf only
/// tracks membership; the miner performs capacity-bounded LRU eviction.
#[derive(Debug)]
pub(crate) struct LeafBucket {
    cap: usize,
    ids: Vec<ClusterId>,
}

impl LeafBucket {
    /// New leaf bounded at `cap` clusters.
    pub(crate) fn new(cap: usize) -> Self {
        LeafBucket {
            cap,
            ids: Vec::new(),
        }
    }

    /// The cluster ids currently in the bucket.
    pub(crate) fn ids(&self) -> &[ClusterId] {
        &self.ids
    }

    /// Whether the bucket is at capacity (a further insert should evict first).
    pub(crate) fn is_full(&self) -> bool {
        self.ids.len() >= self.cap
    }

    /// Add a cluster id (caller ensures capacity via [`Self::is_full`] + eviction).
    pub(crate) fn insert(&mut self, id: ClusterId) {
        self.ids.push(id);
    }

    /// Remove a cluster id if present.
    pub(crate) fn remove(&mut self, id: ClusterId) {
        if let Some(pos) = self.ids.iter().position(|&x| x == id) {
            self.ids.swap_remove(pos);
        }
    }
}

/// A node in a shard's prefix tree.
#[derive(Debug)]
pub(crate) enum TreeNode {
    /// Internal node keyed by token text (or wildcard for numeric positions).
    Internal {
        children: std::collections::HashMap<Arc<str>, TreeNode, FxBuildHasher>,
    },
    /// Terminal node holding the cluster bucket.
    Leaf(LeafBucket),
}

impl TreeNode {
    /// Create an empty internal node.
    pub(crate) fn new_internal() -> Self {
        TreeNode::Internal {
            children: std::collections::HashMap::with_hasher(FxBuildHasher),
        }
    }

    /// Descend `keys` from this node, creating internal nodes and the terminal
    /// leaf (bounded by `leaf_cap`) as needed. Returns a mutable ref to the leaf.
    pub(crate) fn descend_or_create(
        &mut self,
        keys: &[Arc<str>],
        leaf_cap: usize,
    ) -> &mut LeafBucket {
        match keys.split_first() {
            None => {
                // No more keys: this node must be (or become) the leaf.
                if !matches!(self, TreeNode::Leaf(_)) {
                    *self = TreeNode::Leaf(LeafBucket::new(leaf_cap));
                }
                match self {
                    TreeNode::Leaf(b) => b,
                    TreeNode::Internal { .. } => unreachable!(),
                }
            }
            Some((head, rest)) => {
                let children = match self {
                    TreeNode::Internal { children } => children,
                    TreeNode::Leaf(_) => unreachable!("internal/leaf depth is fixed per shard"),
                };
                let next = children.entry(head.clone()).or_insert_with(|| {
                    if rest.is_empty() {
                        TreeNode::Leaf(LeafBucket::new(leaf_cap))
                    } else {
                        TreeNode::new_internal()
                    }
                });
                next.descend_or_create(rest, leaf_cap)
            }
        }
    }

    /// Read-only descent: returns the leaf if the full path exists, else `None`.
    pub(crate) fn descend(&self, keys: &[Arc<str>]) -> Option<&LeafBucket> {
        match keys.split_first() {
            None => match self {
                TreeNode::Leaf(b) => Some(b),
                TreeNode::Internal { .. } => None,
            },
            Some((head, rest)) => match self {
                TreeNode::Internal { children } => children.get(head)?.descend(rest),
                TreeNode::Leaf(_) => None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaf_membership_and_capacity() {
        let mut leaf = LeafBucket::new(2);
        assert!(!leaf.is_full());
        leaf.insert(10);
        leaf.insert(20);
        assert!(leaf.is_full());
        let mut ids = leaf.ids().to_vec();
        ids.sort_unstable();
        assert_eq!(ids, vec![10, 20]);
        leaf.remove(10);
        assert!(!leaf.is_full());
        assert_eq!(leaf.ids(), &[20]);
    }

    #[test]
    fn descend_creates_path_and_returns_leaf() {
        let mut root = TreeNode::new_internal();
        let keys = [Arc::from("GET"), Arc::from("/api")];
        root.descend_or_create(&keys, 100).insert(1);
        // Descending the same path again reaches the same leaf.
        let leaf2 = root.descend_or_create(&keys, 100);
        assert_eq!(leaf2.ids(), &[1]);
    }

    #[test]
    fn descend_readonly_misses_on_absent_path() {
        let mut root = TreeNode::new_internal();
        let keys = [Arc::from("GET")];
        root.descend_or_create(&keys, 100).insert(1);
        let other = [Arc::from("POST")];
        assert!(root.descend(&other).is_none());
        assert!(root.descend(&keys).is_some());
    }

    #[test]
    fn empty_keys_make_root_a_leaf() {
        let mut root = TreeNode::new_internal();
        let keys: [Arc<str>; 0] = [];
        root.descend_or_create(&keys, 5).insert(1);
        assert_eq!(root.descend(&keys).unwrap().ids(), &[1]);
    }
}
