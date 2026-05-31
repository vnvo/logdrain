//! `logdrain` — online Drain3 log-template mining.
//!
//! An incremental log-template miner: feed log lines to a [`Miner`] and it
//! clusters them into templates online, returning a cluster id per line.

mod cluster;
mod error;
mod mask;
mod miner;
mod options;
mod similarity;
mod snapshot;
mod tokenize;
mod tree;

pub use cluster::Cluster;
pub use error::LogdrainError;
pub use mask::{builtin_masks, Mask};
pub use miner::{AddResult, Miner, UpdateType};
pub use options::{MinerBuilder, Options};
pub use tokenize::{OwnedToken, Token};

/// Stable, process-unique identifier for a cluster.
pub type ClusterId = u64;
