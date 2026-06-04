//! The public `Miner`. Token-count sharding with a `RwLock` per shard; cluster
//! bodies in a shared `DashMap` keyed by id.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use rustc_hash::FxBuildHasher;
use smallvec::SmallVec;

use crate::cluster::{Cluster, ClusterInner};
use crate::mask::apply_masks;
use crate::options::Options;
use crate::similarity::similarity;
use crate::snapshot::{decode, encode, ClusterSnapshot, SnapshotV1, TokenSnapshot};
use crate::tokenize::{is_numeric_token, split_first_line, tokenize_with, Token};
use crate::{ClusterId, OwnedToken};

/// How an `add` affected the matched/created cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateType {
    /// A brand-new cluster was created.
    Created,
    /// An existing cluster's template was generalized.
    TemplateChanged,
    /// An existing cluster matched with no template change.
    None,
}

/// Result of [`Miner::add`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddResult {
    /// The cluster the line was assigned to.
    pub cluster_id: ClusterId,
    /// What happened to that cluster.
    pub update: UpdateType,
}

/// One shard: the prefix tree for a single token count, behind its own lock.
struct Shard {
    root: crate::tree::TreeNode,
}

/// Thread-safe online log-template miner.
pub struct Miner {
    shards: DashMap<usize, Arc<RwLock<Shard>>, FxBuildHasher>,
    clusters_by_id: DashMap<ClusterId, Arc<RwLock<ClusterInner>>, FxBuildHasher>,
    options: Arc<Options>,
    counter: AtomicU64,
}

impl Miner {
    /// Build a miner from resolved options.
    pub fn from_options(options: Options) -> Self {
        Miner {
            shards: DashMap::with_hasher(FxBuildHasher),
            clusters_by_id: DashMap::with_hasher(FxBuildHasher),
            options: Arc::new(options),
            counter: AtomicU64::new(0),
        }
    }

    /// Start building a miner with the default builder.
    pub fn builder() -> crate::MinerBuilder {
        crate::MinerBuilder::new()
    }

    /// Number of live clusters.
    pub fn len(&self) -> usize {
        self.clusters_by_id.len()
    }

    /// Whether the miner has no clusters.
    pub fn is_empty(&self) -> bool {
        self.clusters_by_id.is_empty()
    }

    /// Compute the descent keys for a token vector (numeric -> wildcard).
    fn descent_keys(&self, tokens: &[Token<'_>]) -> SmallVec<[Arc<str>; 8]> {
        let n = self.options.prefix_len().min(tokens.len());
        let mut keys: SmallVec<[Arc<str>; 8]> = SmallVec::new();
        for tok in &tokens[..n] {
            if self.options.parametrize_numeric_tokens && is_numeric_token(tok.text) {
                keys.push(self.options.wildcard.clone());
            } else {
                keys.push(Arc::from(tok.text));
            }
        }
        keys
    }

    /// Get the shard for `count`, creating it if absent.
    fn shard_for(&self, count: usize) -> Arc<RwLock<Shard>> {
        if let Some(s) = self.shards.get(&count) {
            return s.clone();
        }
        self.shards
            .entry(count)
            .or_insert_with(|| {
                Arc::new(RwLock::new(Shard {
                    root: crate::tree::TreeNode::new_internal(),
                }))
            })
            .clone()
    }

    /// Ingest a line. Returns the assigned cluster id and what happened.
    pub fn add(&self, line: &str) -> AddResult {
        self.add_inner(line, None)
    }

    /// Ingest a line, recording `member` on the matched/created cluster (deduped).
    pub fn add_with_member(&self, line: &str, member: &str) -> AddResult {
        self.add_inner(line, Some(member))
    }

    /// Shared `add` path: mask -> first-line split -> path tokenize -> match/create.
    ///
    /// Matching (the common case) runs under the shard *read* lock, so adds that
    /// land in the same shard but match existing templates proceed concurrently.
    /// The shard *write* lock is taken only to create a cluster or grow the tree.
    fn add_inner(&self, line: &str, member: Option<&str>) -> AddResult {
        let masked = apply_masks(line, &self.options.masks);
        let (first, suffix) = if self.options.first_line_only {
            split_first_line(&masked)
        } else {
            (masked.as_ref(), None)
        };
        let tokens = tokenize_with(first, self.options.active_path_delimiters());
        let count = tokens.len();
        let keys = self.descent_keys(&tokens);
        let shard = self.shard_for(count);

        // Phase 1 — match under a read lock (no tree mutation, no shard exclusivity).
        let matched = {
            let guard = shard.read().expect("shard lock poisoned");
            guard
                .root
                .descend(&keys)
                .and_then(|leaf| self.best_match(leaf, &tokens))
                .filter(|&(_, sim)| sim >= self.options.sim_threshold)
                .map(|(id, _)| id)
        };
        if let Some(id) = matched {
            // The cluster could be evicted between phases; `apply_match` returns
            // `None` if so, and we fall through to the write phase.
            if let Some(result) = self.apply_match(id, &tokens, member) {
                return result;
            }
        }

        // Phase 2 — create, under the write lock. Holding it prevents concurrent
        // eviction of this shard's clusters, so a re-scan match cannot vanish.
        let mut guard = shard.write().expect("shard lock poisoned");
        let leaf = guard
            .root
            .descend_or_create(&keys, self.options.max_clusters_per_leaf);
        // Re-scan: another writer may have created a matching cluster meanwhile.
        if let Some((id, sim)) = self.best_match(leaf, &tokens) {
            if sim >= self.options.sim_threshold {
                if let Some(result) = self.apply_match(id, &tokens, member) {
                    return result;
                }
            }
        }

        // Genuinely new cluster.
        let id = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let owned: Vec<OwnedToken> = tokens.iter().map(OwnedToken::from).collect();
        let mut inner = ClusterInner::new(id, owned, SystemTime::now(), suffix.map(Arc::from));
        if let Some(m) = member {
            inner.add_member(m);
        }
        self.clusters_by_id.insert(id, Arc::new(RwLock::new(inner)));
        self.evict_if_full(leaf);
        leaf.insert(id);
        AddResult {
            cluster_id: id,
            update: UpdateType::Created,
        }
    }

    /// Highest-similarity cluster in a leaf and its score, reading bodies under
    /// their read locks.
    fn best_match(
        &self,
        leaf: &crate::tree::LeafBucket,
        tokens: &[Token<'_>],
    ) -> Option<(ClusterId, f64)> {
        let mut best: Option<(ClusterId, f64)> = None;
        for &id in leaf.ids() {
            if let Some(arc) = self.clusters_by_id.get(&id) {
                let body = arc.read().expect("cluster lock poisoned");
                let sim = similarity(&body.tokens, tokens, &self.options.wildcard);
                if best.map_or(true, |(_, b)| sim > b) {
                    best = Some((id, sim));
                }
            }
        }
        best
    }

    /// Apply a match to cluster `id`: bump its hit count + recency under the read
    /// lock, taking the write lock only to generalize the template or record a
    /// member. Returns `None` if the cluster was concurrently evicted.
    fn apply_match(
        &self,
        id: ClusterId,
        tokens: &[Token<'_>],
        member: Option<&str>,
    ) -> Option<AddResult> {
        let arc = self.clusters_by_id.get(&id)?.clone();
        let needs_generalize = {
            let body = arc.read().expect("cluster lock poisoned");
            body.touch();
            body.would_generalize(tokens, &self.options.wildcard)
        };
        if !needs_generalize && member.is_none() {
            return Some(AddResult {
                cluster_id: id,
                update: UpdateType::None,
            });
        }
        let mut body = arc.write().expect("cluster lock poisoned");
        let changed = needs_generalize && body.generalize(tokens, &self.options.wildcard);
        if let Some(m) = member {
            body.add_member(m);
        }
        Some(AddResult {
            cluster_id: id,
            update: if changed {
                UpdateType::TemplateChanged
            } else {
                UpdateType::None
            },
        })
    }

    /// Evict the least-recently-used cluster from a full leaf to make room. Called
    /// only under the shard write lock.
    fn evict_if_full(&self, leaf: &mut crate::tree::LeafBucket) {
        if !leaf.is_full() {
            return;
        }
        let victim = leaf.ids().iter().copied().min_by_key(|id| {
            self.clusters_by_id
                .get(id)
                .map(|a| a.read().expect("cluster lock poisoned").recency())
                .unwrap_or(0)
        });
        if let Some(v) = victim {
            leaf.remove(v);
            self.clusters_by_id.remove(&v);
        }
    }

    /// Preprocess a line the same way `add` does, without learning: returns the
    /// first-line token vector (after masking + optional first-line split + path
    /// splitting). The returned tokens borrow `masked`, so the caller keeps it alive.
    fn tokens_for_query<'a>(&self, masked: &'a str) -> crate::tokenize::Tokens<'a> {
        let first = if self.options.first_line_only {
            split_first_line(masked).0
        } else {
            masked
        };
        tokenize_with(first, self.options.active_path_delimiters())
    }

    /// Read-only: return the id of the best cluster at/above threshold, else None.
    /// Does not learn, does not touch LRU recency.
    pub fn match_only(&self, line: &str) -> Option<ClusterId> {
        let masked = apply_masks(line, &self.options.masks);
        let tokens = self.tokens_for_query(&masked);
        let count = tokens.len();
        let keys = self.descent_keys(&tokens);
        let shard = self.shards.get(&count)?.clone();
        let guard = shard.read().expect("shard lock poisoned");
        let leaf = guard.root.descend(&keys)?;
        self.best_match(leaf, &tokens)
            .filter(|&(_, sim)| sim >= self.options.sim_threshold)
            .map(|(id, _)| id)
    }

    /// Match a line and, on a hit, return the captured wildcard-position values
    /// (the incoming token at each position where the template token is wildcard).
    pub fn extract(&self, line: &str) -> Option<(ClusterId, Vec<String>)> {
        let id = self.match_only(line)?;
        let masked = apply_masks(line, &self.options.masks);
        let tokens = self.tokens_for_query(&masked);
        let arc = self.clusters_by_id.get(&id)?.clone();
        let body = arc.read().expect("cluster lock poisoned");
        let mut params = Vec::new();
        for (stored, tok) in body.tokens.iter().zip(tokens.iter()) {
            // Arc<str> PartialEq compares contents; clean and clippy-safe.
            if stored.text == self.options.wildcard {
                params.push(tok.text.to_string());
            }
        }
        Some((id, params))
    }

    /// Snapshot of all clusters (order unspecified).
    pub fn clusters(&self) -> Vec<Cluster> {
        self.clusters_by_id
            .iter()
            .map(|e| {
                e.value()
                    .read()
                    .expect("cluster lock poisoned")
                    .to_public(&self.options.wildcard)
            })
            .collect()
    }

    /// Snapshot of a single cluster by id.
    pub fn cluster(&self, id: ClusterId) -> Option<Cluster> {
        let arc = self.clusters_by_id.get(&id)?.clone();
        let body = arc.read().expect("cluster lock poisoned");
        Some(body.to_public(&self.options.wildcard))
    }

    /// Insert an already-constructed cluster body into the tree + id map.
    /// Used by `restore`. Assumes `id` is not already present.
    fn insert_existing(&self, inner: ClusterInner) {
        let count = inner.tokens.len();
        // Build a borrowed token view for descent-key computation, scoped so its
        // borrow of `inner.tokens` ends before `inner` is moved below. `keys` is
        // owned (Arc<str>), so it outlives the view.
        let keys = {
            let view: SmallVec<[Token<'_>; 16]> = inner
                .tokens
                .iter()
                .map(|t| Token {
                    text: &t.text,
                    leading_delim: t.leading_delim,
                    trailing_delim: t.trailing_delim,
                })
                .collect();
            self.descent_keys(&view)
        };
        let id = inner.id;
        let shard = self.shard_for(count);
        let mut guard = shard.write().expect("shard lock poisoned");
        let leaf = guard
            .root
            .descend_or_create(&keys, self.options.max_clusters_per_leaf);
        self.clusters_by_id.insert(id, Arc::new(RwLock::new(inner)));
        self.evict_if_full(leaf);
        leaf.insert(id);
    }

    /// Serialize miner state (options + counter + flat cluster list) to bytes.
    pub fn snapshot(&self) -> Vec<u8> {
        let clusters = self
            .clusters_by_id
            .iter()
            .map(|e| {
                let b = e.value().read().expect("cluster lock poisoned");
                ClusterSnapshot {
                    id: b.id,
                    tokens: b
                        .tokens
                        .iter()
                        .map(|t| TokenSnapshot {
                            text: t.text.to_string(),
                            leading_delim: t.leading_delim,
                            trailing_delim: t.trailing_delim,
                        })
                        .collect(),
                    size: b.size.load(Ordering::Relaxed),
                    created_at_ms: system_time_to_ms(b.created_at),
                    updated_at_ms: b.updated_at_ms.load(Ordering::Relaxed),
                    suffix: b.suffix.as_ref().map(|s| s.to_string()),
                    members: b.members.iter().map(|m| m.to_string()).collect(),
                }
            })
            .collect();
        let body = SnapshotV1 {
            options: (*self.options).clone(),
            counter: self.counter.load(Ordering::Relaxed),
            clusters,
        };
        encode(&body)
    }

    /// Replace miner state from a snapshot. Clears existing clusters first.
    ///
    /// `self.options` is intentionally NOT mutated (it is set at construction and
    /// `add`/descent depend on it). The snapshot carries options for self-description
    /// and forward-compat; constructing the destination miner with matching options
    /// is the caller's responsibility.
    pub fn restore(&self, bytes: &[u8]) -> Result<(), crate::LogdrainError> {
        let body = decode(bytes)?;
        self.shards.clear();
        self.clusters_by_id.clear();
        self.counter.store(body.counter, Ordering::Relaxed);
        for cs in body.clusters {
            let tokens: Vec<OwnedToken> = cs
                .tokens
                .into_iter()
                .map(|t| OwnedToken {
                    text: Arc::from(t.text.as_str()),
                    leading_delim: t.leading_delim,
                    trailing_delim: t.trailing_delim,
                })
                .collect();
            let inner = ClusterInner {
                id: cs.id,
                tokens,
                size: AtomicU64::new(cs.size),
                created_at: ms_to_system_time(cs.created_at_ms),
                updated_at_ms: AtomicU64::new(cs.updated_at_ms),
                suffix: cs.suffix.map(Arc::from),
                members: cs.members.into_iter().map(Arc::from).collect(),
            };
            self.insert_existing(inner);
        }
        Ok(())
    }

    /// Serialize the miner and store it via the given backend.
    pub fn save_state(&self, p: &dyn crate::Persistence) -> Result<(), crate::LogdrainError> {
        p.save(&self.snapshot())?;
        Ok(())
    }

    /// Load state from the backend, replacing current state. Returns `false` if the
    /// backend held nothing (miner left unchanged), `true` if a snapshot was loaded.
    pub fn load_state(&self, p: &dyn crate::Persistence) -> Result<bool, crate::LogdrainError> {
        match p.load()? {
            Some(bytes) => {
                self.restore(&bytes)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

fn system_time_to_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ms_to_system_time(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

impl crate::MinerBuilder {
    /// Validate options and construct a [`Miner`].
    pub fn build(self) -> Result<Miner, crate::LogdrainError> {
        Ok(Miner::from_options(self.build_options()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MinerBuilder;

    fn miner() -> Miner {
        Miner::from_options(MinerBuilder::new().build_options().unwrap())
    }

    #[test]
    fn first_line_creates_cluster() {
        let m = miner();
        let r = m.add("user 42 logged in");
        assert_eq!(r.update, UpdateType::Created);
        assert_eq!(r.cluster_id, 1);
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn similar_line_joins_and_generalizes() {
        let m = miner();
        let a = m.add("user 42 logged in");
        let b = m.add("user 99 logged in");
        assert_eq!(b.cluster_id, a.cluster_id);
        assert_eq!(b.update, UpdateType::TemplateChanged);
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn identical_line_is_update_none() {
        let m = miner();
        m.add("a b c d e");
        let r = m.add("a b c d e");
        assert_eq!(r.update, UpdateType::None);
    }

    #[test]
    fn different_token_count_makes_new_cluster() {
        let m = miner();
        let a = m.add("a b c");
        let b = m.add("a b c d");
        assert_ne!(a.cluster_id, b.cluster_id);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn dissimilar_same_length_makes_new_cluster() {
        let m = miner();
        // Default threshold 0.4, 5 tokens: differing in 4/5 -> sim 0.2 < 0.4.
        let a = m.add("alpha one two three four");
        let b = m.add("alpha NINE TEN ELEVEN TWELVE");
        assert_ne!(a.cluster_id, b.cluster_id);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn numeric_parametrization_groups_by_wildcard_prefix() {
        // With numeric parametrization, leading numbers in the prefix don't
        // fragment the tree: these two share a descent path.
        let m = miner();
        let a = m.add("100 ms elapsed for request");
        let b = m.add("200 ms elapsed for request");
        assert_eq!(a.cluster_id, b.cluster_id);
    }

    #[test]
    fn ids_are_monotonic() {
        let m = miner();
        let a = m.add("a b c");
        let b = m.add("x y z");
        assert_eq!(a.cluster_id, 1);
        assert_eq!(b.cluster_id, 2);
    }

    #[test]
    fn builder_build_constructs_miner() {
        let m = MinerBuilder::new().sim_threshold(0.5).build().unwrap();
        assert_eq!(m.len(), 0);
        assert!(MinerBuilder::new().depth(1).build().is_err());
    }

    #[test]
    fn match_only_finds_without_learning() {
        let m = miner();
        let a = m.add("user 42 logged in");
        let before = m.len();
        let hit = m.match_only("user 7 logged in");
        assert_eq!(hit, Some(a.cluster_id));
        assert_eq!(m.len(), before); // no new cluster, size unchanged
        assert_eq!(m.cluster(a.cluster_id).unwrap().size(), 1);
    }

    #[test]
    fn match_only_misses_return_none() {
        let m = miner();
        m.add("a b c");
        assert_eq!(m.match_only("x y z w"), None); // different token count
        assert_eq!(m.match_only("p q r"), None); // same count, no path
    }

    #[test]
    fn extract_returns_wildcard_values() {
        let m = miner();
        m.add("user 42 logged in");
        m.add("user 99 logged in"); // generalizes slot 1 to <*>
        let (id, params) = m.extract("user 7 logged in").unwrap();
        assert_eq!(id, m.match_only("user 7 logged in").unwrap());
        assert_eq!(params, vec!["7".to_string()]);
    }

    #[test]
    fn clusters_and_cluster_snapshots() {
        let m = miner();
        let a = m.add("a b c");
        m.add("x y z");
        let all = m.clusters();
        assert_eq!(all.len(), 2);
        let one = m.cluster(a.cluster_id).unwrap();
        assert_eq!(one.id(), a.cluster_id);
        assert!(m.cluster(99999).is_none());
    }

    fn miner_with(b: MinerBuilder) -> Miner {
        Miner::from_options(b.build_options().unwrap())
    }

    #[test]
    fn path_clustering_preserves_structure() {
        let m = miner_with(MinerBuilder::new().path_delimiters(&['/']));
        let a = m.add("PUT /servers/409/foo/10.0.0.1");
        let b = m.add("PUT /servers/410/foo/10.0.0.2");
        assert_eq!(a.cluster_id, b.cluster_id);
        assert_eq!(
            m.cluster(a.cluster_id).unwrap().template(),
            "PUT /servers/<*>/foo/<*>"
        );
    }

    #[test]
    fn masks_cluster_high_cardinality_tokens() {
        let m = miner_with(MinerBuilder::new().masks([crate::builtin_masks::uuid()]));
        let a = m.add("request 550e8400-e29b-41d4-a716-446655440000 ok");
        let b = m.add("request 6ba7b810-9dad-11d1-80b4-00c04fd430c8 ok");
        assert_eq!(a.cluster_id, b.cluster_id);
        assert_eq!(b.update, UpdateType::None); // identical after masking
        assert_eq!(
            m.cluster(a.cluster_id).unwrap().template(),
            "request <uuid> ok"
        );
    }

    #[test]
    fn first_line_only_captures_suffix() {
        let m = miner_with(MinerBuilder::new().first_line_only(true));
        let a = m.add("NullPointerException at Foo\n  at bar()\n  at baz()");
        assert_eq!(
            m.cluster(a.cluster_id).unwrap().suffix(),
            Some("  at bar()\n  at baz()")
        );
        // Same first line, different stack -> same cluster; suffix stays the first one.
        let b = m.add("NullPointerException at Foo\n  at other()");
        assert_eq!(a.cluster_id, b.cluster_id);
        assert_eq!(
            m.cluster(a.cluster_id).unwrap().suffix(),
            Some("  at bar()\n  at baz()")
        );
    }

    #[test]
    fn add_with_member_records_deduped_members() {
        let m = miner();
        let a = m.add_with_member("user 1 logged in", "svc-a");
        m.add_with_member("user 2 logged in", "svc-b");
        m.add_with_member("user 3 logged in", "svc-a"); // duplicate member
        let c = m.cluster(a.cluster_id).unwrap();
        let members: Vec<&str> = c.members().iter().map(|m| &**m).collect();
        assert_eq!(members, vec!["svc-a", "svc-b"]);
        // Plain add records no member.
        let d = m.add("totally different shape here now");
        assert!(m.cluster(d.cluster_id).unwrap().members().is_empty());
    }

    #[test]
    fn extract_honors_masks_and_path() {
        let m = miner_with(MinerBuilder::new().path_delimiters(&['/']));
        m.add("GET /u/409/x");
        m.add("GET /u/410/x"); // generalize middle to <*>
        let (_, params) = m.extract("GET /u/777/x").unwrap();
        assert_eq!(params, vec!["777".to_string()]);
    }
}
