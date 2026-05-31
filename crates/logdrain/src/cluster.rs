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
    /// Verbatim remainder after the first line (set at creation in first-line mode).
    pub(crate) suffix: Option<Arc<str>>,
    /// Deduplicated member labels recorded via `add_with_member`.
    pub(crate) members: Vec<Arc<str>>,
}

impl ClusterInner {
    /// Create a fresh cluster of size 1 from the given owned tokens and optional suffix.
    pub(crate) fn new(
        id: ClusterId,
        tokens: Vec<OwnedToken>,
        now: SystemTime,
        suffix: Option<Arc<str>>,
    ) -> Self {
        ClusterInner {
            id,
            tokens,
            size: 1,
            created_at: now,
            updated_at: now,
            suffix,
            members: Vec::new(),
        }
    }

    /// Record a member label, de-duplicating against existing members.
    pub(crate) fn add_member(&mut self, member: &str) {
        if !self.members.iter().any(|m| &**m == member) {
            self.members.push(Arc::from(member));
        }
    }

    /// Render the template string. Path-joined sub-tokens (where the previous
    /// token has a trailing delimiter) are joined by that delimiter with no
    /// space; otherwise tokens are space-separated. A token's own leading
    /// delimiter is emitted as a prefix only when the previous token did not
    /// already supply the joining delimiter.
    pub(crate) fn render_template(&self, _wildcard: &str) -> String {
        let mut s = String::new();
        let mut prev_trailing: Option<char> = None;
        for (i, t) in self.tokens.iter().enumerate() {
            if i > 0 {
                match prev_trailing {
                    Some(c) => s.push(c),
                    None => s.push(' '),
                }
            }
            if prev_trailing.is_none() {
                if let Some(c) = t.leading_delim {
                    s.push(c);
                }
            }
            s.push_str(&t.text);
            prev_trailing = t.trailing_delim;
        }
        if let Some(c) = prev_trailing {
            s.push(c);
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
                // Replace the text but KEEP the delimiter flags so path structure
                // is preserved (e.g. `/servers/<*>/foo`).
                stored.text = Arc::from(wildcard);
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
            suffix: self.suffix.clone(),
            members: self.members.clone(),
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
    suffix: Option<Arc<str>>,
    members: Vec<Arc<str>>,
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
    /// Verbatim suffix captured in first-line-only mode, if any.
    pub fn suffix(&self) -> Option<&str> {
        self.suffix.as_deref()
    }
    /// Deduplicated member labels recorded via `add_with_member`.
    pub fn members(&self) -> &[Arc<str>] {
        &self.members
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
        ClusterInner::new(id, toks, SystemTime::UNIX_EPOCH, None)
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

    use crate::tokenize::tokenize_with;

    fn inner_path(line: &str, id: u64) -> ClusterInner {
        let toks: Vec<_> = tokenize_with(line, &['/'])
            .iter()
            .map(crate::OwnedToken::from)
            .collect();
        ClusterInner::new(id, toks, SystemTime::UNIX_EPOCH, None)
    }

    #[test]
    fn suffix_is_exposed() {
        let toks: Vec<_> = tokenize("boom")
            .iter()
            .map(crate::OwnedToken::from)
            .collect();
        let c = ClusterInner::new(
            1,
            toks,
            SystemTime::UNIX_EPOCH,
            Some(Arc::from("at line 1\nat line 2")),
        );
        assert_eq!(c.to_public("<*>").suffix(), Some("at line 1\nat line 2"));
    }

    #[test]
    fn members_dedup() {
        let mut c = inner_from("a b", 1);
        c.add_member("svc-a");
        c.add_member("svc-b");
        c.add_member("svc-a"); // duplicate ignored
        let snap = c.to_public("<*>");
        let members: Vec<&str> = snap.members().iter().map(|m| &**m).collect();
        assert_eq!(members, vec!["svc-a", "svc-b"]);
    }

    #[test]
    fn render_round_trips_path_template() {
        let c = inner_path("/servers/409/foo", 1);
        assert_eq!(c.render_template("<*>"), "/servers/409/foo");
    }

    #[test]
    fn render_round_trips_mixed() {
        let c = inner_path("GET /servers/409 ok", 1);
        assert_eq!(c.render_template("<*>"), "GET /servers/409 ok");
    }

    #[test]
    fn render_round_trips_trailing_delim() {
        let c = inner_path("dir/", 1);
        assert_eq!(c.render_template("<*>"), "dir/");
    }

    #[test]
    fn generalize_preserves_path_structure() {
        let mut c = inner_path("/servers/409/foo", 1);
        let incoming = tokenize_with("/servers/410/foo", &['/']);
        assert!(c.generalize(&incoming, "<*>"));
        assert_eq!(c.render_template("<*>"), "/servers/<*>/foo");
    }

    #[test]
    fn generalize_path_is_idempotent() {
        let mut c = inner_path("/servers/409/foo", 1);
        assert!(c.generalize(&tokenize_with("/servers/410/foo", &['/']), "<*>"));
        assert!(!c.generalize(&tokenize_with("/servers/999/foo", &['/']), "<*>"));
        assert_eq!(c.render_template("<*>"), "/servers/<*>/foo");
    }
}
