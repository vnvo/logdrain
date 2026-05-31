//! Mine templates from a noisy, realistic log stream and show how raw lines
//! collapse into a handful of templates. Demonstrates path-preserving
//! tokenization, UUID/IPv4/email masking, numeric parametrization, and
//! parameter extraction. Run with: `cargo run -p logdrain --example basic`

use logdrain::{builtin_masks, Miner};

fn main() {
    let miner = Miner::builder()
        .path_delimiters(&['/'])
        .masks([
            builtin_masks::uuid(),
            builtin_masks::ipv4(),
            builtin_masks::email(),
        ])
        .build()
        .unwrap();

    // A burst of access logs, request traces, signups, and cache events — the
    // kind of high-cardinality noise you'd see scrolling past in production.
    let logs = [
        "GET /api/v1/servers/409/metrics 200 12ms",
        "GET /api/v1/servers/410/metrics 200 9ms",
        "GET /api/v1/servers/873/metrics 200 15ms",
        "GET /api/v1/servers/409/metrics 503 41ms",
        "POST /api/v1/users/5512/login 200 from 10.0.0.7",
        "POST /api/v1/users/9920/login 200 from 10.14.2.3",
        "POST /api/v1/users/3001/login 401 from 192.168.1.9",
        "request 550e8400-e29b-41d4-a716-446655440000 completed in 88ms",
        "request 6ba7b810-9dad-11d1-80b4-00c04fd430c8 completed in 102ms",
        "request 7c9e6679-7425-40de-944b-e07fc1f90ae7 completed in 75ms",
        "signup for alice@example.com from 10.0.0.7",
        "signup for bob@corp.io from 203.0.113.5",
        "signup for carol@mail.net from 198.51.100.2",
        "cache miss for key user:5512:profile",
        "cache miss for key user:9920:profile",
        "cache miss for key order:88123:items",
    ];

    for line in logs {
        miner.add(line);
    }

    let mut clusters = miner.clusters();
    clusters.sort_by(|a, b| b.size().cmp(&a.size()).then(a.id().cmp(&b.id())));
    let total: u64 = clusters.iter().map(|c| c.size()).sum();

    println!(
        "{total} raw lines  ->  {} templates  ({:.1}x compression)\n",
        clusters.len(),
        total as f64 / clusters.len() as f64,
    );
    println!("{:>3}  {:>4}  TEMPLATE", "ID", "SIZE");
    println!("{}", "-".repeat(64));
    for c in &clusters {
        println!("{:>3}  {:>4}  {}", c.id(), c.size(), c.template());
    }

    // Match a brand-new line against learned templates and pull out the
    // variable parts (the positions that generalized to the wildcard).
    let probe = "GET /api/v1/servers/999/metrics 200 7ms";
    if let Some((id, params)) = miner.extract(probe) {
        println!("\nextract({probe:?})");
        println!("  matched cluster #{id}, captured params: {params:?}");
    }
}
