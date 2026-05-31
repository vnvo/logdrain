//! Prefix tree within a shard + per-leaf LRU bucket.

use std::num::NonZeroUsize;
use std::sync::Arc;

use lru::LruCache;
use rustc_hash::FxBuildHasher;

use crate::ClusterId;

/// Leaf bucket: an LRU set of cluster ids. Bodies live in the miner's
/// `clusters_by_id` map; this only tracks membership + recency.
#[derive(Debug)]
pub(crate) struct LeafBucket {
    clusters: LruCache<ClusterId, ()>,
}

impl LeafBucket {
    /// New leaf bounded at `cap` clusters (`cap >= 1`).
    pub(crate) fn new(cap: usize) -> Self {
        let cap = NonZeroUsize::new(cap).expect("cap must be >= 1");
        LeafBucket {
            clusters: LruCache::new(cap),
        }
    }

    /// Insert a cluster id as most-recently-used. If insertion evicts the
    /// least-recently-used id (bucket was full), returns the evicted id.
    pub(crate) fn insert(&mut self, id: ClusterId) -> Option<ClusterId> {
        // `push` returns the evicted (key, value) when at capacity.
        match self.clusters.push(id, ()) {
            Some((evicted, ())) if evicted != id => Some(evicted),
            _ => None,
        }
    }

    /// Mark a cluster id most-recently-used.
    pub(crate) fn touch(&mut self, id: ClusterId) {
        let _ = self.clusters.get(&id);
    }

    /// All cluster ids currently in the bucket (order unspecified).
    pub(crate) fn ids(&self) -> Vec<ClusterId> {
        self.clusters.iter().map(|(k, _)| *k).collect()
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
    fn leaf_inserts_and_lists() {
        let mut leaf = LeafBucket::new(3);
        assert!(leaf.insert(10).is_none());
        assert!(leaf.insert(20).is_none());
        let mut ids = leaf.ids();
        ids.sort_unstable();
        assert_eq!(ids, vec![10, 20]);
    }

    #[test]
    fn leaf_evicts_lru_when_full() {
        let mut leaf = LeafBucket::new(2);
        assert_eq!(leaf.insert(1), None);
        assert_eq!(leaf.insert(2), None);
        // touch 1 so 2 is now least-recently-used
        leaf.touch(1);
        let evicted = leaf.insert(3);
        assert_eq!(evicted, Some(2));
        let mut ids = leaf.ids();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 3]);
    }

    #[test]
    fn descend_creates_path_and_returns_leaf() {
        let mut root = TreeNode::new_internal();
        let keys = [Arc::from("GET"), Arc::from("/api")];
        let leaf = root.descend_or_create(&keys, 100);
        assert!(leaf.insert(1).is_none());
        // Descending the same path again reaches the same leaf.
        let leaf2 = root.descend_or_create(&keys, 100);
        assert_eq!(leaf2.ids(), vec![1]);
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
        let leaf = root.descend_or_create(&keys, 5);
        assert!(leaf.insert(1).is_none());
        assert_eq!(root.descend(&keys).unwrap().ids(), vec![1]);
    }
}
