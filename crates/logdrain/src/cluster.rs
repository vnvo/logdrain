//! Cluster types. `ClusterInner` is the mutable body held by the miner;
//! `Cluster` is an immutable snapshot returned to callers.

use std::sync::Arc;
use std::time::SystemTime;

use crate::tokenize::Token;
use crate::{ClusterId, OwnedToken};

/// Mutable cluster body. Stored as `Arc<RwLock<ClusterInner>>` in the miner.
#[derive(Debug)]
pub(crate) struct ClusterInner {
    pub(crate) id: ClusterId,
    pub(crate) tokens: Vec<OwnedToken>,
    pub(crate) size: u64,
    pub(crate) created_at: SystemTime,
    pub(crate) updated_at: SystemTime,
}

impl ClusterInner {
    /// Create a fresh cluster of size 1 from the given owned tokens.
    pub(crate) fn new(id: ClusterId, tokens: Vec<OwnedToken>, now: SystemTime) -> Self {
        ClusterInner {
            id,
            tokens,
            size: 1,
            created_at: now,
            updated_at: now,
        }
    }

    /// Render the template string by joining token texts with single spaces.
    /// (v0.1 has no path delimiters, so no special delimiter join yet.)
    pub(crate) fn render_template(&self, _wildcard: &str) -> String {
        let mut s = String::new();
        for (i, t) in self.tokens.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            s.push_str(&t.text);
        }
        s
    }

    /// Generalize the template against an incoming token vector of equal length:
    /// any position whose stored token differs from the incoming token (and is
    /// not already the wildcard) becomes the wildcard. Returns whether anything
    /// changed. Caller guarantees equal length (same shard).
    pub(crate) fn generalize(&mut self, incoming: &[Token<'_>], wildcard: &str) -> bool {
        debug_assert_eq!(self.tokens.len(), incoming.len());
        let mut changed = false;
        for (stored, tok) in self.tokens.iter_mut().zip(incoming.iter()) {
            if &*stored.text == wildcard {
                continue;
            }
            if &*stored.text != tok.text {
                stored.text = Arc::from(wildcard);
                stored.leading_delim = None;
                stored.trailing_delim = None;
                changed = true;
            }
        }
        changed
    }

    /// Produce an immutable public snapshot.
    pub(crate) fn to_public(&self, wildcard: &str) -> Cluster {
        Cluster {
            id: self.id,
            template: self.render_template(wildcard),
            tokens: self.tokens.clone(),
            size: self.size,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// Immutable snapshot of a cluster, returned by miner query APIs.
#[derive(Debug, Clone)]
pub struct Cluster {
    id: ClusterId,
    template: String,
    tokens: Vec<OwnedToken>,
    size: u64,
    created_at: SystemTime,
    updated_at: SystemTime,
}

impl Cluster {
    /// Stable cluster id.
    pub fn id(&self) -> ClusterId {
        self.id
    }
    /// Number of lines that have joined this cluster.
    pub fn size(&self) -> u64 {
        self.size
    }
    /// Rendered template string (path-aware join lands in v0.2).
    pub fn template(&self) -> &str {
        &self.template
    }
    /// The template's owned tokens.
    pub fn tokens(&self) -> &[OwnedToken] {
        &self.tokens
    }
    /// Stack-trace suffix — always `None` in v0.1 (lands in v0.2).
    pub fn suffix(&self) -> Option<&str> {
        None
    }
    /// Deduplicated members — always empty in v0.1 (lands in v0.2).
    pub fn members(&self) -> &[Arc<str>] {
        &[]
    }
    /// When the cluster was first created.
    pub fn created_at(&self) -> SystemTime {
        self.created_at
    }
    /// When the cluster was last updated.
    pub fn updated_at(&self) -> SystemTime {
        self.updated_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenize::tokenize;
    use std::time::SystemTime;

    fn inner_from(line: &str, id: u64) -> ClusterInner {
        let toks: Vec<_> = tokenize(line).iter().map(crate::OwnedToken::from).collect();
        ClusterInner::new(id, toks, SystemTime::UNIX_EPOCH)
    }

    #[test]
    fn template_joins_with_spaces() {
        let c = inner_from("GET /api 200", 1);
        assert_eq!(c.render_template("<*>"), "GET /api 200");
    }

    #[test]
    fn generalize_replaces_differing_tokens() {
        let mut c = inner_from("user 42 logged in", 1);
        let incoming = tokenize("user 99 logged in");
        let changed = c.generalize(&incoming, "<*>");
        assert!(changed);
        assert_eq!(c.render_template("<*>"), "user <*> logged in");
    }

    #[test]
    fn generalize_is_idempotent() {
        let mut c = inner_from("user 42 logged in", 1);
        let incoming = tokenize("user 99 logged in");
        assert!(c.generalize(&incoming, "<*>"));
        // Same shape again: already wildcard at the differing slot -> no change.
        let again = tokenize("user 7 logged in");
        assert!(!c.generalize(&again, "<*>"));
        assert_eq!(c.render_template("<*>"), "user <*> logged in");
    }

    #[test]
    fn generalize_no_diff_returns_false() {
        let mut c = inner_from("a b c", 1);
        let incoming = tokenize("a b c");
        assert!(!c.generalize(&incoming, "<*>"));
    }

    #[test]
    fn snapshot_exposes_accessors() {
        let c = inner_from("a b", 7);
        let snap = c.to_public("<*>");
        assert_eq!(snap.id(), 7);
        assert_eq!(snap.size(), 1);
        assert_eq!(snap.template(), "a b");
        assert_eq!(snap.tokens().len(), 2);
        assert!(snap.suffix().is_none());
        assert!(snap.members().is_empty());
    }
}
