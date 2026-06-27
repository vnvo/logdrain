//! Concurrency scaling: ingest the same corpus from 1, 2, 4, ... threads into a
//! single shared `Miner` and report aggregate throughput and speedup. Because the
//! tree is sharded by token count, lines of differing lengths land in different
//! shards and proceed in parallel; same-length lines contend on one shard lock.
//!
//! Run (release is essential):
//!     cargo run --release -p logdrain --example scaling
//!     cargo run --release -p logdrain --example scaling -- 4000000

use std::thread;
use std::time::Instant;

use logdrain::Miner;

struct Rng(u64);
impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x % n
    }
}

/// Generate a line with a varying token count so the corpus spreads across many
/// token-count shards (otherwise every line contends on a single shard lock).
/// Ids/values are drawn from small spaces so that, once the tree is warm, lines
/// match existing templates — this measures steady-state throughput, not the
/// cold-start churn of unbounded-cardinality input.
fn gen_line(rng: &mut Rng) -> String {
    let mut s = match rng.below(4) {
        0 => format!("GET /svc/{}/metrics 200", rng.below(200)),
        1 => format!("POST /svc/{}/login 200", rng.below(200)),
        2 => format!("cache miss key:{}", rng.below(200)),
        _ => format!("worker {} done", rng.below(64)),
    };
    // 0..16 trailing key=value tokens -> 16 distinct token counts per shape.
    for j in 0..rng.below(16) {
        s.push_str(&format!(" k{j}={}", rng.below(8)));
    }
    s
}

fn ingest_chunks(miner: &Miner, corpus: &[String], threads: usize) -> std::time::Duration {
    let chunk = corpus.len().div_ceil(threads);
    let start = Instant::now();
    thread::scope(|scope| {
        for part in corpus.chunks(chunk) {
            scope.spawn(move || {
                for line in part {
                    miner.add(line);
                }
            });
        }
    });
    start.elapsed()
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(2_000_000);

    let cores = thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4);
    print!("generating {n} lines... ");
    let mut rng = Rng(0xC0FF_EE00_1234_5678);
    let corpus: Vec<String> = (0..n).map(|_| gen_line(&mut rng)).collect();
    println!("done  ({cores} logical cores available)\n");

    // Thread counts: powers of two up to the core count.
    let mut thread_counts = vec![1];
    let mut t = 2;
    while t <= cores {
        thread_counts.push(t);
        t *= 2;
    }

    println!(
        "{:>7}  {:>9}  {:>14}  {:>8}",
        "THREADS", "TIME", "LINES/SEC", "SPEEDUP"
    );
    println!("{}", "-".repeat(46));
    let mut baseline = 0.0f64;
    for &threads in &thread_counts {
        let miner = Miner::builder().path_delimiters(&['/']).build().unwrap();
        // Warm the tree single-threaded (untimed) so the timed pass measures
        // steady-state matching, not one-time cluster creation.
        for line in &corpus {
            miner.add(line);
        }
        let elapsed = ingest_chunks(&miner, &corpus, threads);
        let per_sec = n as f64 / elapsed.as_secs_f64();
        if threads == 1 {
            baseline = per_sec;
        }
        println!(
            "{:>7}  {:>9.2?}  {:>14.0}  {:>7.2}x",
            threads,
            elapsed,
            per_sec,
            per_sec / baseline
        );
    }
}
