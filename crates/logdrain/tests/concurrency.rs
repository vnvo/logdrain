//! Correctness under concurrent ingestion: many threads hammering one miner must
//! not lose updates, panic, or deadlock.

use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use logdrain::{Miner, UpdateType};

/// Four pairwise-dissimilar 4-token lines: all land in the same token-count shard
/// (maximizing write-lock contention) but in distinct leaves -> exactly 4 clusters.
const LINES: [&str; 4] = [
    "user logged in successfully",
    "disk usage above threshold",
    "connection reset by peer",
    "payment processed for order",
];

#[test]
fn concurrent_adds_lose_no_updates() {
    const THREADS: usize = 8;
    const PER_LINE_PER_THREAD: u64 = 20_000;

    // Large per-leaf cap so nothing is evicted (sizes must sum exactly).
    let miner = Miner::builder()
        .max_clusters_per_leaf(1_000)
        .build()
        .unwrap();

    thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|| {
                for _ in 0..PER_LINE_PER_THREAD {
                    for line in LINES {
                        miner.add(line);
                    }
                }
            });
        }
    });

    // Exactly the four distinct shapes, each hit by every thread.
    assert_eq!(miner.len(), LINES.len());
    let total: u64 = miner.clusters().iter().map(|c| c.size()).sum();
    assert_eq!(
        total,
        THREADS as u64 * PER_LINE_PER_THREAD * LINES.len() as u64
    );
    for c in miner.clusters() {
        assert_eq!(c.size(), THREADS as u64 * PER_LINE_PER_THREAD);
    }
    for line in LINES {
        assert!(miner.match_only(line).is_some());
    }
}

#[test]
fn concurrent_mixed_add_and_match_does_not_deadlock() {
    const WRITERS: usize = 4;
    const READERS: usize = 4;
    const ITERS: u64 = 50_000;

    let miner = Miner::builder().build().unwrap();
    miner.add(LINES[0]); // seed so readers can hit something
    let reads = AtomicU64::new(0);

    thread::scope(|scope| {
        for _ in 0..WRITERS {
            scope.spawn(|| {
                for _ in 0..ITERS {
                    for line in LINES {
                        miner.add(line);
                    }
                }
            });
        }
        for _ in 0..READERS {
            scope.spawn(|| {
                for _ in 0..ITERS {
                    if miner.match_only(LINES[0]).is_some() {
                        reads.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
    });

    // If we got here, no deadlock. Reads ran and the seeded line stays matchable.
    assert!(reads.load(Ordering::Relaxed) > 0);
    let r = miner.add(LINES[0]);
    assert_ne!(r.update, UpdateType::Created); // already known
}
