//! Snapshot format: `MAGIC + VERSION + bincode(SnapshotV1)`.
//! The tree is not serialized; it is rebuilt by replaying clusters on restore.

use serde::{Deserialize, Serialize};

use crate::options::Options;

/// 8-byte file magic.
pub(crate) const MAGIC: &[u8; 8] = b"LOGDRAIN";
/// Current snapshot version. Bumped to 3 because event-time fields
/// (`event_first_ms` / `event_last_ms`) were added to `ClusterSnapshot`. bincode is
/// not self-describing, so a positional layout change requires a new version;
/// `decode` upgrades version 2 in place and rejects anything else cleanly.
pub(crate) const VERSION: u32 = 3;

/// A single serialized cluster (current, v3). Tokens stored as plain strings to
/// avoid the serde `rc` feature.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ClusterSnapshot {
    pub id: u64,
    pub tokens: Vec<TokenSnapshot>,
    pub size: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    /// Smallest event timestamp (unix-ms), or [`crate::cluster::EVENT_UNSET`].
    pub event_first_ms: u64,
    /// Largest event timestamp (unix-ms), or `0` when unset.
    pub event_last_ms: u64,
    /// Verbatim first-line-mode suffix.
    pub suffix: Option<String>,
    /// Deduplicated member labels.
    pub members: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TokenSnapshot {
    pub text: String,
    pub leading_delim: Option<char>,
    pub trailing_delim: Option<char>,
}

/// Versioned snapshot body (current, v3).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SnapshotV1 {
    pub options: Options,
    pub counter: u64,
    pub clusters: Vec<ClusterSnapshot>,
}

/// v2 cluster layout (no event-time fields), kept only to read older snapshots.
/// `Serialize` is derived for tests that forge a v2 blob.
#[derive(Debug, Serialize, Deserialize)]
struct ClusterSnapshotV2 {
    id: u64,
    tokens: Vec<TokenSnapshot>,
    size: u64,
    created_at_ms: u64,
    updated_at_ms: u64,
    suffix: Option<String>,
    members: Vec<String>,
}

/// v2 body layout. `Options` is unchanged between v2 and v3, so only the cluster
/// shape differs.
#[derive(Debug, Serialize, Deserialize)]
struct SnapshotBodyV2 {
    options: Options,
    counter: u64,
    clusters: Vec<ClusterSnapshotV2>,
}

impl SnapshotBodyV2 {
    /// Upgrade a v2 body to the current layout, defaulting event time to unset.
    fn upgrade(self) -> SnapshotV1 {
        SnapshotV1 {
            options: self.options,
            counter: self.counter,
            clusters: self
                .clusters
                .into_iter()
                .map(|c| ClusterSnapshot {
                    id: c.id,
                    tokens: c.tokens,
                    size: c.size,
                    created_at_ms: c.created_at_ms,
                    updated_at_ms: c.updated_at_ms,
                    event_first_ms: crate::cluster::EVENT_UNSET,
                    event_last_ms: 0,
                    suffix: c.suffix,
                    members: c.members,
                })
                .collect(),
        }
    }
}

/// Encode a body with the magic + version header.
pub(crate) fn encode(body: &SnapshotV1) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    let payload = bincode::serialize(body).expect("bincode serialize cannot fail for owned data");
    out.extend_from_slice(&payload);
    out
}

/// Validate header and decode the body.
pub(crate) fn decode(bytes: &[u8]) -> Result<SnapshotV1, crate::LogdrainError> {
    if bytes.len() < MAGIC.len() + 4 {
        return Err(crate::LogdrainError::BadMagic);
    }
    if &bytes[..MAGIC.len()] != MAGIC {
        return Err(crate::LogdrainError::BadMagic);
    }
    let mut v = [0u8; 4];
    v.copy_from_slice(&bytes[MAGIC.len()..MAGIC.len() + 4]);
    let found = u32::from_le_bytes(v);
    let payload = &bytes[MAGIC.len() + 4..];
    // bincode is positional, so each version must be decoded with its own layout.
    // v3 is current; v2 is upgraded in place; anything else is rejected cleanly.
    match found {
        VERSION => {
            bincode::deserialize(payload).map_err(|e| crate::LogdrainError::Decode(e.to_string()))
        }
        2 => bincode::deserialize::<SnapshotBodyV2>(payload)
            .map(SnapshotBodyV2::upgrade)
            .map_err(|e| crate::LogdrainError::Decode(e.to_string())),
        _ => Err(crate::LogdrainError::UnsupportedSnapshotVersion {
            found,
            max: VERSION,
        }),
    }
}

#[cfg(test)]
mod tests {
    use crate::Miner;
    use crate::MinerBuilder;

    fn miner() -> Miner {
        Miner::from_options(MinerBuilder::new().build_options().unwrap())
    }

    #[test]
    fn round_trip_preserves_clusters_and_ids() {
        let m = miner();
        m.add("user 42 logged in");
        m.add("user 99 logged in");
        m.add("disk full on /dev/sda");
        let next_id_before = m.add("a brand new shape here").cluster_id;

        let bytes = m.snapshot();

        let m2 = miner();
        m2.restore(&bytes).unwrap();
        assert_eq!(m2.len(), m.len());

        // The generalized template survived.
        let id = m2.match_only("user 7 logged in").unwrap();
        assert_eq!(m2.cluster(id).unwrap().template(), "user <*> logged in");

        // Timestamps survive the round-trip (to millisecond precision).
        let ms =
            |t: std::time::SystemTime| t.duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
        let before = m.cluster(id).unwrap();
        let after_c = m2.cluster(id).unwrap();
        assert_eq!(ms(before.created_at()), ms(after_c.created_at()));
        assert_eq!(ms(before.updated_at()), ms(after_c.updated_at()));
        assert!(
            ms(after_c.created_at()) > 0,
            "timestamp must be a real epoch-ms value"
        );

        // Counter survived: the next new cluster id is strictly greater.
        let after = m2.add("yet another different shape entirely").cluster_id;
        assert!(after > next_id_before, "{after} !> {next_id_before}");
    }

    #[test]
    fn bad_magic_is_rejected() {
        let m = miner();
        let err = m.restore(b"NOTMAGIC....").unwrap_err();
        assert!(matches!(err, crate::LogdrainError::BadMagic));
    }

    #[test]
    fn short_input_is_rejected() {
        let m = miner();
        assert!(m.restore(b"AB").is_err());
    }

    #[test]
    fn future_version_is_rejected() {
        // MAGIC + version 999 + empty body.
        let mut bytes = crate::snapshot::MAGIC.to_vec();
        bytes.extend_from_slice(&999u32.to_le_bytes());
        let err = miner().restore(&bytes).unwrap_err();
        assert!(matches!(
            err,
            crate::LogdrainError::UnsupportedSnapshotVersion { found: 999, .. }
        ));
    }

    #[test]
    fn older_version_is_rejected() {
        // A v0.1 (version 1) blob predates the readable layouts and is rejected.
        let mut bytes = crate::snapshot::MAGIC.to_vec();
        bytes.extend_from_slice(&1u32.to_le_bytes());
        let err = miner().restore(&bytes).unwrap_err();
        assert!(matches!(
            err,
            crate::LogdrainError::UnsupportedSnapshotVersion { found: 1, .. }
        ));
    }

    #[test]
    fn reads_v2_snapshot_and_defaults_event_time_unset() {
        use super::{ClusterSnapshotV2, SnapshotBodyV2, TokenSnapshot, MAGIC};
        let tok = |t: &str| TokenSnapshot {
            text: t.to_string(),
            leading_delim: None,
            trailing_delim: None,
        };
        let body = SnapshotBodyV2 {
            options: crate::MinerBuilder::new().build_options().unwrap(),
            counter: 9,
            clusters: vec![ClusterSnapshotV2 {
                id: 1,
                tokens: vec![tok("user"), tok("<*>"), tok("in")],
                size: 3,
                created_at_ms: 1000,
                updated_at_ms: 2000,
                suffix: None,
                members: vec![],
            }],
        };
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&bincode::serialize(&body).unwrap());

        let m = miner();
        m.restore(&bytes).unwrap(); // v2 read succeeds
        assert_eq!(m.len(), 1);
        let id = m.match_only("user 7 in").unwrap();
        let c = m.cluster(id).unwrap();
        assert_eq!(c.template(), "user <*> in");
        assert!(c.event_first_seen().is_none()); // no event time in v2 -> unset
                                                 // Counter survived: next id is greater than the restored counter.
        assert!(m.add("a totally different shape entirely now").cluster_id > 9);
    }

    #[test]
    fn round_trip_preserves_suffix_and_members() {
        let m = Miner::from_options(
            MinerBuilder::new()
                .path_delimiters(&['/'])
                .first_line_only(true)
                .build_options()
                .unwrap(),
        );
        m.add_with_member("GET /servers/409/foo\ntrace line a\ntrace line b", "svc-a");
        m.add_with_member("GET /servers/410/foo\ntrace line c", "svc-b");

        let bytes = m.snapshot();
        let m2 = Miner::from_options(
            MinerBuilder::new()
                .path_delimiters(&['/'])
                .first_line_only(true)
                .build_options()
                .unwrap(),
        );
        m2.restore(&bytes).unwrap();

        assert_eq!(m2.len(), 1);
        let id = m2.match_only("GET /servers/999/foo").unwrap();
        let c = m2.cluster(id).unwrap();
        assert_eq!(c.template(), "GET /servers/<*>/foo");
        // Suffix captured at creation (from the first line) survives.
        assert_eq!(c.suffix(), Some("trace line a\ntrace line b"));
        let members: Vec<&str> = c.members().iter().map(|m| &**m).collect();
        assert_eq!(members, vec!["svc-a", "svc-b"]);
    }
}
