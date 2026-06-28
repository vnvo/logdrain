//! Functional tests for `KafkaPersistence` against a real Kafka broker started with
//! testcontainers (Apache Kafka, KRaft mode). Requires Docker; run with `--ignored`:
//!
//!   cargo test -p logdrain --features kafka --test kafka_backend -- --ignored
//!
//! The `kafka` feature builds librdkafka, so this only compiles where that toolchain
//! is available (it runs in CI's `optional backends` job).
#![cfg(feature = "kafka")]

use logdrain::{KafkaPersistence, Miner, Persistence};
use testcontainers::runners::SyncRunner;
use testcontainers_modules::kafka::apache;

/// Start a broker and return it plus its `host:port` bootstrap string. The broker is
/// advertised on 127.0.0.1 at the mapped port (per the testcontainers apache module).
fn broker() -> (impl Drop, String) {
    let node = apache::Kafka::default()
        .start()
        .expect("start kafka container");
    let port = node
        .get_host_port_ipv4(apache::KAFKA_PORT)
        .expect("mapped kafka port");
    (node, format!("127.0.0.1:{port}"))
}

#[test]
#[ignore = "requires Docker (testcontainers)"]
fn kafka_round_trips_a_miner_snapshot() {
    let (_node, brokers) = broker();
    let store = KafkaPersistence::new(brokers, "logdrain-test-snapshots");

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
fn kafka_load_returns_the_latest_snapshot() {
    let (_node, brokers) = broker();
    let store = KafkaPersistence::new(brokers, "logdrain-test-latest");

    store.save(b"first").unwrap();
    store.save(b"second").unwrap();
    // load reads the tail of the partition, so the most recent write wins.
    assert_eq!(store.load().unwrap(), Some(b"second".to_vec()));
}
