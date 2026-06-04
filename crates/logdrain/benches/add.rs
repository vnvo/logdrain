//! Criterion benchmarks for the hot paths: steady-state add, cold-start add, and
//! the read-only match path. Run with `cargo bench -p logdrain`.

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use logdrain::Miner;

/// Deterministic xorshift64 so benchmark inputs are reproducible.
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

fn line(rng: &mut Rng) -> String {
    match rng.below(4) {
        0 => format!(
            "GET /api/v1/servers/{}/metrics 200 {}ms",
            rng.below(100_000),
            rng.below(500)
        ),
        1 => format!("POST /api/v1/users/{}/login 200 ok", rng.below(100_000)),
        2 => format!("cache miss for key user:{}:profile", rng.below(100_000)),
        _ => format!(
            "worker {} processed batch in {}ms",
            rng.below(64),
            rng.below(900)
        ),
    }
}

fn build_miner() -> Miner {
    Miner::builder().path_delimiters(&['/']).build().unwrap()
}

fn bench_add_steady_state(c: &mut Criterion) {
    // Pre-warm so the tree is populated; then measure adds into the warm tree.
    let miner = build_miner();
    let mut rng = Rng(0x1234_5678_9abc_def0);
    for _ in 0..2_000 {
        miner.add(&line(&mut rng));
    }
    c.bench_function("add_steady_state", |b| {
        b.iter_batched(
            || line(&mut rng),
            |l| {
                miner.add(&l);
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_add_cold_start(c: &mut Criterion) {
    let mut rng = Rng(0xfeed_face_dead_beef);
    c.bench_function("add_cold_start", |b| {
        b.iter_batched(
            || {
                // Fresh miner + a distinct line every iteration (every add is new).
                (build_miner(), line(&mut rng))
            },
            |(miner, l)| {
                miner.add(&l);
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_match_only(c: &mut Criterion) {
    let miner = build_miner();
    let mut rng = Rng(0x0bad_c0de_cafe_f00d);
    for _ in 0..2_000 {
        miner.add(&line(&mut rng));
    }
    c.bench_function("match_only", |b| {
        b.iter_batched(
            || line(&mut rng),
            |l| miner.match_only(&l),
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    benches,
    bench_add_steady_state,
    bench_add_cold_start,
    bench_match_only
);
criterion_main!(benches);
