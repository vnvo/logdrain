//! Cluster types. `ClusterInner` is the mutable body held by the miner;
//! `Cluster` is an immutable snapshot returned to callers.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::tokenize::Token;
use crate::{ClusterId, OwnedToken};

/// Unix-millis for a `SystemTime` (saturating to 0 before the epoch).
pub(crate) fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `SystemTime` from unix-millis.
pub(crate) fn time_from_ms(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

fn now_ms() -> u64 {
    unix_ms(SystemTime::now())
}

/// Mutable cluster body. Stored as `Arc<RwLock<ClusterInner>>` in the miner.
///
/// `size`, `updated_at_ms`, and `last_used` are atomics so the hot match path can
/// bump a hit through a shared (read) lock without taking the exclusive cluster
/// lock; `tokens` and `members` change rarely and stay behind the `RwLock`'s
/// write guard. `last_used` is a monotonic tick (not wall-clock) used as the LRU
/// recency key, so eviction order is precise even when many hits land in the same
/// millisecond.
#[derive(Debug)]
pub(crate) struct ClusterInner {
    pub(crate) id: ClusterId,
    pub(crate) tokens: Vec<OwnedToken>,
    pub(crate) size: AtomicU64,
    pub(crate) created_at: SystemTime,
    pub(crate) updated_at_ms: AtomicU64,
    pub(crate) last_used: AtomicU64,
    /// Smallest event timestamp (unix-ms) seen for this cluster, or [`EVENT_UNSET`]
    /// if no event time has been supplied (via `add_at`). Event time is what the
    /// caller parses out of the log; it is independent of `created_at`/`updated_at`.
    pub(crate) event_first_ms: AtomicU64,
    /// Largest event timestamp (unix-ms) seen, or `0` when unset.
    pub(crate) event_last_ms: AtomicU64,
    /// Remainder after the first line (set at creation in first-line mode); it goes
    /// through the same mask pass as the rest of the input, so it is not raw text.
    pub(crate) suffix: Option<Arc<str>>,
    /// Deduplicated member labels recorded via `add_with_member`.
    pub(crate) members: Vec<Arc<str>>,
}

/// Sentinel for `event_first_ms` meaning "no event timestamp recorded". Using
/// `u64::MAX` lets [`ClusterInner::observe_event`] use a plain atomic `fetch_min`.
pub(crate) const EVENT_UNSET: u64 = u64::MAX;

impl ClusterInner {
    /// Create a fresh cluster of size 1 from the given owned tokens and optional
    /// suffix. `event_ms` is the caller-supplied event timestamp (unix-ms) for this
    /// first line, or `None` if event time is not being tracked.
    pub(crate) fn new(
        id: ClusterId,
        tokens: Vec<OwnedToken>,
        now: SystemTime,
        suffix: Option<Arc<str>>,
        event_ms: Option<u64>,
    ) -> Self {
        let (event_first, event_last) = match event_ms {
            Some(ms) => (ms, ms),
            None => (EVENT_UNSET, 0),
        };
        ClusterInner {
            id,
            tokens,
            size: AtomicU64::new(1),
            created_at: now,
            updated_at_ms: AtomicU64::new(unix_ms(now)),
            last_used: AtomicU64::new(0),
            event_first_ms: AtomicU64::new(event_first),
            event_last_ms: AtomicU64::new(event_last),
            suffix,
            members: Vec::new(),
        }
    }

    /// Record a hit: increment size, refresh the wall-clock `updated_at`, and set
    /// the LRU recency to `tick` (a monotonic value supplied by the miner). Safe
    /// through a shared reference (atomics), so callers need only the read lock.
    pub(crate) fn touch(&self, tick: u64) {
        self.size.fetch_add(1, Ordering::Relaxed);
        self.updated_at_ms.store(now_ms(), Ordering::Relaxed);
        self.last_used.store(tick, Ordering::Relaxed);
    }

    /// Fold an event timestamp (unix-ms) into this cluster's min/max event window.
    /// Atomic, so it runs under the shared (read) lock like [`touch`](Self::touch).
    pub(crate) fn observe_event(&self, ms: u64) {
        self.event_first_ms.fetch_min(ms, Ordering::Relaxed);
        self.event_last_ms.fetch_max(ms, Ordering::Relaxed);
    }

    /// LRU recency key (monotonic tick; higher = more recently used).
    pub(crate) fn recency(&self) -> u64 {
        self.last_used.load(Ordering::Relaxed)
    }

    /// Whether generalizing against `incoming` would change the template. Read-only,
    /// so the match path can decide if the exclusive lock is needed at all.
    pub(crate) fn would_generalize(&self, incoming: &[Token<'_>], wildcard: &str) -> bool {
        self.tokens
            .iter()
            .zip(incoming.iter())
            .any(|(stored, tok)| &*stored.text != wildcard && &*stored.text != tok.text)
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
        let ef = self.event_first_ms.load(Ordering::Relaxed);
        let (event_first, event_last) = if ef == EVENT_UNSET {
            (None, None)
        } else {
            (
                Some(time_from_ms(ef)),
                Some(time_from_ms(self.event_last_ms.load(Ordering::Relaxed))),
            )
        };
        Cluster {
            id: self.id,
            template: self.render_template(wildcard),
            tokens: self.tokens.clone(),
            size: self.size.load(Ordering::Relaxed),
            created_at: self.created_at,
            updated_at: time_from_ms(self.updated_at_ms.load(Ordering::Relaxed)),
            event_first,
            event_last,
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
    event_first: Option<SystemTime>,
    event_last: Option<SystemTime>,
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
    /// Suffix captured in first-line-only mode, if any (masked like the rest of the
    /// input, not raw text). See [`crate::MinerBuilder::first_line_only`].
    pub fn suffix(&self) -> Option<&str> {
        self.suffix.as_deref()
    }
    /// Deduplicated member labels recorded via `add_with_member`.
    pub fn members(&self) -> &[Arc<str>] {
        &self.members
    }
    /// Wall-clock time this cluster was first observed **during processing**.
    ///
    /// This is the machine's clock at the moment `add` first created the cluster -
    /// it is **not** parsed from the log line. logdrain never reads timestamps out
    /// of your records. Streaming a live feed, this approximates event time; replaying
    /// a historical file, it is the time you ran the program, not when the events
    /// happened.
    pub fn created_at(&self) -> SystemTime {
        self.created_at
    }
    /// Wall-clock time this cluster was last hit **during processing** (same caveat
    /// as [`created_at`](Self::created_at): observation time, not log time).
    pub fn updated_at(&self) -> SystemTime {
        self.updated_at
    }
    /// Approximate lines-per-minute over the cluster's **observed** lifetime
    /// (`updated_at - created_at`). Returns `0.0` when the span is under one second
    /// (rate is unknown over a sub-second window - e.g. a one-shot batch where all
    /// lines arrive at once).
    ///
    /// Because the span is measured from processing wall-clock (see
    /// [`created_at`](Self::created_at)), this is the rate logdrain *saw* lines, which
    /// only equals the real event rate when you stream a live feed. Replaying a file
    /// measures how fast you fed it, not the original traffic.
    pub fn lines_per_minute(&self) -> f64 {
        let secs = self
            .updated_at
            .duration_since(self.created_at)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        if secs < 1.0 {
            0.0
        } else {
            self.size as f64 / (secs / 60.0)
        }
    }

    /// Earliest **event** timestamp recorded for this cluster, or `None` if event
    /// time was never supplied (i.e. lines were added with `add`, not `add_at`).
    ///
    /// Unlike [`created_at`](Self::created_at), this is the timestamp *you parsed
    /// from the log* and passed in - so it reflects when the event actually
    /// happened, even when replaying a historical file.
    pub fn event_first_seen(&self) -> Option<SystemTime> {
        self.event_first
    }

    /// Latest **event** timestamp recorded, or `None` if event time was never
    /// supplied. See [`event_first_seen`](Self::event_first_seen).
    pub fn event_last_seen(&self) -> Option<SystemTime> {
        self.event_last
    }

    /// True lines-per-minute computed from the **event** time window
    /// (`event_last_seen - event_first_seen`), or `None` if event time was never
    /// supplied. Returns `Some(0.0)` when the window is under one second (e.g. a
    /// single event, or many sharing one timestamp).
    ///
    /// This is the event-rate analogue of [`lines_per_minute`](Self::lines_per_minute):
    /// it measures real traffic, not how fast logdrain processed the input.
    pub fn event_lines_per_minute(&self) -> Option<f64> {
        let (first, last) = (self.event_first?, self.event_last?);
        let secs = last
            .duration_since(first)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        Some(if secs < 1.0 {
            0.0
        } else {
            self.size as f64 / (secs / 60.0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenize::tokenize;
    use std::time::SystemTime;

    fn inner_from(line: &str, id: u64) -> ClusterInner {
        let toks: Vec<_> = tokenize(line).iter().map(crate::OwnedToken::from).collect();
        ClusterInner::new(id, toks, SystemTime::UNIX_EPOCH, None, None)
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
        ClusterInner::new(id, toks, SystemTime::UNIX_EPOCH, None, None)
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
            None,
        );
        assert_eq!(c.to_public("<*>").suffix(), Some("at line 1\nat line 2"));
    }

    #[test]
    fn event_time_unset_by_default() {
        let snap = inner_from("a b", 1).to_public("<*>");
        assert!(snap.event_first_seen().is_none());
        assert!(snap.event_last_seen().is_none());
        assert!(snap.event_lines_per_minute().is_none());
    }

    #[test]
    fn observe_event_tracks_min_max_and_rate() {
        let toks: Vec<_> = tokenize("a b")
            .iter()
            .map(crate::OwnedToken::from)
            .collect();
        // Created at event t=60_000 ms; later observe earlier and later events.
        let c = ClusterInner::new(1, toks, SystemTime::UNIX_EPOCH, None, Some(60_000));
        c.observe_event(0); // earlier -> new min
        c.observe_event(180_000); // later -> new max (span 180s = 3 min)
        c.size.store(6, Ordering::Relaxed);
        let snap = c.to_public("<*>");
        assert_eq!(snap.event_first_seen(), Some(time_from_ms(0)));
        assert_eq!(snap.event_last_seen(), Some(time_from_ms(180_000)));
        assert_eq!(snap.event_lines_per_minute(), Some(2.0)); // 6 lines / 3 min
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
    fn lines_per_minute_zero_on_short_span() {
        // Instant span (created == updated) -> 0.0 (rate unknown), not infinity.
        let c = inner_from("a b", 1);
        c.size.store(5, Ordering::Relaxed);
        assert_eq!(c.to_public("<*>").lines_per_minute(), 0.0);
    }

    #[test]
    fn lines_per_minute_over_two_minutes() {
        let c = inner_from("a b", 1); // created_at = UNIX_EPOCH
        c.size.store(120, Ordering::Relaxed);
        c.updated_at_ms.store(120_000, Ordering::Relaxed); // 120s after the epoch
        assert_eq!(c.to_public("<*>").lines_per_minute(), 60.0); // 120 lines / 2 min
    }

    #[test]
    fn generalize_path_is_idempotent() {
        let mut c = inner_path("/servers/409/foo", 1);
        assert!(c.generalize(&tokenize_with("/servers/410/foo", &['/']), "<*>"));
        assert!(!c.generalize(&tokenize_with("/servers/999/foo", &['/']), "<*>"));
        assert_eq!(c.render_template("<*>"), "/servers/<*>/foo");
    }
}
