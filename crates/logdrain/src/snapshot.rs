//! Snapshot format: `MAGIC + VERSION + bincode(SnapshotV1)`.
//! The tree is not serialized; it is rebuilt by replaying clusters on restore.

use serde::{Deserialize, Serialize};

use crate::options::Options;

/// 8-byte file magic.
pub(crate) const MAGIC: &[u8; 8] = b"LOGDRAIN";
/// Current snapshot version.
pub(crate) const VERSION: u32 = 1;

/// A single serialized cluster. Tokens stored as plain strings to avoid the
/// serde `rc` feature. Delimiters are reserved for v0.2 (always None now).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ClusterSnapshot {
    pub id: u64,
    pub tokens: Vec<TokenSnapshot>,
    pub size: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TokenSnapshot {
    pub text: String,
    pub leading_delim: Option<char>,
    pub trailing_delim: Option<char>,
}

/// Versioned snapshot body.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SnapshotV1 {
    pub options: Options,
    pub counter: u64,
    pub clusters: Vec<ClusterSnapshot>,
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
    if found > VERSION {
        return Err(crate::LogdrainError::UnsupportedSnapshotVersion {
            found,
            max: VERSION,
        });
    }
    let payload = &bytes[MAGIC.len() + 4..];
    bincode::deserialize(payload).map_err(|e| crate::LogdrainError::Decode(e.to_string()))
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
}
