//! Pluggable persistence: a small synchronous trait plus in-tree Memory and File
//! backends. The miner serializes via [`crate::Miner::snapshot`] and hands the
//! bytes to a backend; the interface is intentionally minimal so other backends
//! (Redis, S3) can live in separate crates.

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
}
