//! Error type for the `logdrain` crate.

/// Errors returned by `logdrain`. v0.1 subset; persistence/regex variants land
/// in later phases.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LogdrainError {
    /// A builder option failed validation.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_config_displays_message() {
        let e = LogdrainError::InvalidConfig("depth must be >= 2".into());
        assert_eq!(e.to_string(), "invalid configuration: depth must be >= 2");
    }

    #[test]
    fn bad_magic_displays() {
        assert_eq!(
            LogdrainError::BadMagic.to_string(),
            "corrupt snapshot: bad magic"
        );
    }

    #[test]
    fn unsupported_version_displays() {
        let e = LogdrainError::UnsupportedSnapshotVersion { found: 9, max: 1 };
        assert_eq!(e.to_string(), "snapshot version 9 not supported (max 1)");
    }
}
