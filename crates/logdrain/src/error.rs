//! Error type for the `logdrain` crate.

/// Errors returned by `logdrain`. v0.1 subset; persistence/regex variants land
/// in later phases.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LogdrainError {
    /// A builder option failed validation.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    /// A mask regex failed to compile.
    #[error("regex compile failed: {0}")]
    RegexCompile(#[from] regex::Error),

    /// A persistence backend failed.
    #[error("persistence: {0}")]
    Persistence(#[from] crate::persistence::PersistenceError),

    /// Snapshot bytes did not start with the expected magic.
    #[error("corrupt snapshot: bad magic")]
    BadMagic,

    /// Snapshot version is newer than this build understands.
    #[error("snapshot version {found} not supported (max {max})")]
    UnsupportedSnapshotVersion { found: u32, max: u32 },

    /// bincode failed to decode the snapshot body.
    #[error("corrupt snapshot: {0}")]
    Decode(String),
}
