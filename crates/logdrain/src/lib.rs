//! `logdrain` — online log-template mining.
//!
//! An independent Rust implementation of the **Drain** algorithm (He et al., 2017),
//! with the practical extensions Drain3 popularized (masking, persistence, numeric
//! parametrization). Feed log lines to a [`Miner`] and it clusters them into templates
//! online, returning a cluster id per line.

mod cluster;
mod error;
mod mask;
mod miner;
mod options;
mod persistence;
mod similarity;
mod snapshot;
mod tokenize;
mod tree;

pub use cluster::Cluster;
pub use error::LogdrainError;
pub use mask::{builtin_masks, Mask};
pub use miner::{AddResult, Miner, UpdateType};
pub use options::{MinerBuilder, Options};
#[cfg(feature = "kafka")]
pub use persistence::KafkaPersistence;
#[cfg(feature = "redis")]
pub use persistence::RedisPersistence;
pub use persistence::{FilePersistence, MemoryPersistence, Persistence, PersistenceError};
pub use tokenize::{OwnedToken, Token};

/// Stable, process-unique identifier for a cluster.
pub type ClusterId = u64;
