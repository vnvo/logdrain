//! Functional tests for `RedisPersistence` against a real Redis started with
//! testcontainers. Requires Docker; run with `--ignored`:
//!
//!   cargo test -p logdrain --features redis --test redis_backend -- --ignored
#![cfg(feature = "redis")]

use logdrain::{Miner, Persistence, RedisPersistence};
use testcontainers::runners::SyncRunner;
use testcontainers_modules::redis::Redis;

/// Default Redis port inside the container.
const REDIS_PORT: u16 = 6379;

fn redis_url() -> (impl Drop, String) {
    let container = Redis::default().start().expect("start redis container");
    let host = container.get_host().expect("container host");
    let port = container
        .get_host_port_ipv4(REDIS_PORT)
        .expect("mapped redis port");
    let url = format!("redis://{host}:{port}/");
    (container, url)
}

#[test]
#[ignore = "requires Docker (testcontainers)"]
fn redis_round_trips_a_miner_snapshot() {
    let (_container, url) = redis_url();
    let store = RedisPersistence::new(&url, "logdrain:test:snapshot").unwrap();

    // Nothing stored yet.
    assert_eq!(store.load().unwrap(), None);

    // Train a miner, persist it, restore into a fresh one.
    let m = Miner::builder().build().unwrap();
    m.add("user 42 logged in");
    m.add("user 99 logged in"); // generalizes slot 1 -> <*>
    m.save_state(&store).unwrap();

    let restored = Miner::builder().build().unwrap();
    assert!(restored.load_state(&store).unwrap());
    assert_eq!(restored.len(), m.len());
    let id = restored.match_only("user 7 logged in").unwrap();
    assert_eq!(
        restored.cluster(id).unwrap().template(),
        "user <*> logged in"
    );
}

#[test]
#[ignore = "requires Docker (testcontainers)"]
fn redis_save_overwrites_previous_blob() {
    let (_container, url) = redis_url();
    let store = RedisPersistence::new(&url, "logdrain:test:raw").unwrap();

    store.save(b"first").unwrap();
    assert_eq!(store.load().unwrap(), Some(b"first".to_vec()));
    store.save(b"second").unwrap();
    assert_eq!(store.load().unwrap(), Some(b"second".to_vec()));
}
