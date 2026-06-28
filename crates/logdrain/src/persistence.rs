//! Pluggable persistence: a small synchronous trait plus in-tree backends. The
//! miner serializes via [`crate::Miner::snapshot`] and hands the bytes to a backend.
//! `MemoryPersistence` and `FilePersistence` are always available; `RedisPersistence`
//! and `KafkaPersistence` are behind the `redis` and `kafka` cargo features so the
//! core stays dependency-light.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Error from a persistence backend.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    /// Underlying I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Failure reported by an external backend (Redis, Kafka, ...).
    #[error("backend: {0}")]
    Backend(String),
}

/// A place to store and retrieve a serialized miner snapshot.
pub trait Persistence {
    /// Persist a snapshot blob, replacing any previous one.
    fn save(&self, blob: &[u8]) -> Result<(), PersistenceError>;
    /// Load the most recent snapshot blob, or `None` if nothing is stored.
    fn load(&self) -> Result<Option<Vec<u8>>, PersistenceError>;
}

/// In-memory backend. Useful for tests and ephemeral processes.
#[derive(Debug, Default)]
pub struct MemoryPersistence {
    blob: Mutex<Option<Vec<u8>>>,
}

impl MemoryPersistence {
    /// Create an empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Persistence for MemoryPersistence {
    fn save(&self, blob: &[u8]) -> Result<(), PersistenceError> {
        *self.blob.lock().expect("memory persistence lock poisoned") = Some(blob.to_vec());
        Ok(())
    }

    fn load(&self) -> Result<Option<Vec<u8>>, PersistenceError> {
        Ok(self
            .blob
            .lock()
            .expect("memory persistence lock poisoned")
            .clone())
    }
}

/// File backend with crash-safe writes: data is written to a temporary sibling
/// file, fsynced, then atomically renamed over the target.
#[derive(Debug, Clone)]
pub struct FilePersistence {
    path: PathBuf,
}

impl FilePersistence {
    /// Persist to `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        FilePersistence { path: path.into() }
    }

    fn tmp_path(&self) -> PathBuf {
        let mut s = self.path.clone().into_os_string();
        s.push(".tmp");
        PathBuf::from(s)
    }
}

impl Persistence for FilePersistence {
    fn save(&self, blob: &[u8]) -> Result<(), PersistenceError> {
        let tmp = self.tmp_path();
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(blob)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn load(&self) -> Result<Option<Vec<u8>>, PersistenceError> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Redis snapshot backend (cargo feature `redis`): stores the latest snapshot blob
/// under a single key. Pure-Rust client, no system dependencies.
#[cfg(feature = "redis")]
mod redis_backend {
    use super::{Persistence, PersistenceError};
    use redis::Commands;

    fn err(e: redis::RedisError) -> PersistenceError {
        PersistenceError::Backend(e.to_string())
    }

    /// Stores the snapshot at a single Redis key (overwritten on each `save`).
    pub struct RedisPersistence {
        client: redis::Client,
        key: String,
    }

    impl RedisPersistence {
        /// Connect to `url` (e.g. `redis://127.0.0.1/`) and store the snapshot at `key`.
        /// The connection is opened lazily per operation, so this only validates the URL.
        pub fn new(url: &str, key: impl Into<String>) -> Result<Self, PersistenceError> {
            let client = redis::Client::open(url).map_err(err)?;
            Ok(Self {
                client,
                key: key.into(),
            })
        }
    }

    impl Persistence for RedisPersistence {
        fn save(&self, blob: &[u8]) -> Result<(), PersistenceError> {
            let mut conn = self.client.get_connection().map_err(err)?;
            conn.set::<_, _, ()>(self.key.as_str(), blob).map_err(err)?;
            Ok(())
        }

        fn load(&self) -> Result<Option<Vec<u8>>, PersistenceError> {
            let mut conn = self.client.get_connection().map_err(err)?;
            let blob: Option<Vec<u8>> = conn.get(self.key.as_str()).map_err(err)?;
            Ok(blob)
        }
    }
}

#[cfg(feature = "redis")]
pub use redis_backend::RedisPersistence;

/// Kafka snapshot backend (cargo feature `kafka`): writes the snapshot as the latest
/// record on a topic and reads it back from the tail of partition 0. Use a
/// single-partition, log-compacted topic so the latest snapshot is retained.
///
/// Built on `rdkafka` (which links librdkafka); enabling this feature requires a C
/// toolchain. Uses the synchronous `BaseProducer` / `BaseConsumer`, so it fits the
/// blocking [`Persistence`] trait without an async runtime.
#[cfg(feature = "kafka")]
mod kafka_backend {
    use std::time::Duration;

    use rdkafka::config::ClientConfig;
    use rdkafka::consumer::{BaseConsumer, Consumer};
    use rdkafka::producer::{BaseProducer, BaseRecord, Producer};
    use rdkafka::{Message, Offset, TopicPartitionList};

    use super::{Persistence, PersistenceError};

    /// Fixed record key the snapshot is written under (so log compaction keeps one).
    const SNAPSHOT_KEY: &str = "logdrain-snapshot";

    fn err<E: std::fmt::Display>(e: E) -> PersistenceError {
        PersistenceError::Backend(e.to_string())
    }

    /// Stores the snapshot as the tail record of a (recommended: compacted,
    /// single-partition) Kafka topic.
    pub struct KafkaPersistence {
        brokers: String,
        topic: String,
        timeout: Duration,
    }

    impl KafkaPersistence {
        /// `brokers` is a comma-separated `host:port` list; `topic` should be a
        /// single-partition, `cleanup.policy=compact` topic.
        pub fn new(brokers: impl Into<String>, topic: impl Into<String>) -> Self {
            Self {
                brokers: brokers.into(),
                topic: topic.into(),
                timeout: Duration::from_secs(10),
            }
        }

        /// Override the per-operation timeout (default 10s).
        pub fn with_timeout(mut self, timeout: Duration) -> Self {
            self.timeout = timeout;
            self
        }
    }

    impl Persistence for KafkaPersistence {
        fn save(&self, blob: &[u8]) -> Result<(), PersistenceError> {
            let producer: BaseProducer = ClientConfig::new()
                .set("bootstrap.servers", self.brokers.as_str())
                .create()
                .map_err(err)?;
            producer
                .send(
                    BaseRecord::to(self.topic.as_str())
                        .key(SNAPSHOT_KEY)
                        .payload(blob),
                )
                .map_err(|(e, _)| err(e))?;
            producer.flush(self.timeout).map_err(err)?;
            Ok(())
        }

        fn load(&self) -> Result<Option<Vec<u8>>, PersistenceError> {
            let consumer: BaseConsumer = ClientConfig::new()
                .set("bootstrap.servers", self.brokers.as_str())
                .set("group.id", "logdrain-loader")
                .set("enable.auto.commit", "false")
                .create()
                .map_err(err)?;
            let (low, high) = consumer
                .fetch_watermarks(self.topic.as_str(), 0, self.timeout)
                .map_err(err)?;
            if high <= low {
                return Ok(None); // nothing stored yet
            }
            let mut tpl = TopicPartitionList::new();
            tpl.add_partition_offset(self.topic.as_str(), 0, Offset::Offset(high - 1))
                .map_err(err)?;
            consumer.assign(&tpl).map_err(err)?;
            match consumer.poll(self.timeout) {
                Some(Ok(msg)) => Ok(msg.payload().map(<[u8]>::to_vec)),
                Some(Err(e)) => Err(err(e)),
                None => Ok(None), // timed out with nothing read
            }
        }
    }
}

#[cfg(feature = "kafka")]
pub use kafka_backend::KafkaPersistence;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_round_trip() {
        let p = MemoryPersistence::new();
        assert_eq!(p.load().unwrap(), None);
        p.save(b"hello").unwrap();
        assert_eq!(p.load().unwrap(), Some(b"hello".to_vec()));
        p.save(b"world").unwrap();
        assert_eq!(p.load().unwrap(), Some(b"world".to_vec()));
    }

    #[test]
    fn file_round_trip() {
        let dir = std::env::temp_dir().join(format!("logdrain-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.bin");
        let p = FilePersistence::new(&path);

        assert_eq!(p.load().unwrap(), None); // missing -> None
        p.save(b"snapshot-bytes").unwrap();
        assert_eq!(p.load().unwrap(), Some(b"snapshot-bytes".to_vec()));
        p.save(b"newer").unwrap(); // overwrite
        assert_eq!(p.load().unwrap(), Some(b"newer".to_vec()));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn miner_save_load_round_trips_via_backend() {
        let m = crate::Miner::builder().build().unwrap();
        m.add("user 42 logged in");
        m.add("user 99 logged in");
        let store = MemoryPersistence::new();
        m.save_state(&store).unwrap();

        let m2 = crate::Miner::builder().build().unwrap();
        assert!(m2.load_state(&store).unwrap()); // true: snapshot present
        assert_eq!(m2.len(), m.len());
        let id = m2.match_only("user 7 logged in").unwrap();
        assert_eq!(m2.cluster(id).unwrap().template(), "user <*> logged in");
    }

    #[test]
    fn load_state_from_empty_backend_is_false() {
        let m = crate::Miner::builder().build().unwrap();
        assert!(!m.load_state(&MemoryPersistence::new()).unwrap());
    }

    // Construction-only unit test; full round-trips live in the `tests/` directory and
    // run a real Redis/Kafka via testcontainers.
    #[cfg(feature = "redis")]
    #[test]
    fn redis_new_validates_url() {
        assert!(RedisPersistence::new("redis://127.0.0.1/", "logdrain:snap").is_ok());
        assert!(RedisPersistence::new("not-a-redis-url", "k").is_err());
    }
}
