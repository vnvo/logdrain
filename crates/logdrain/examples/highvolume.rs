//! High-volume demo: generate a large, realistic log stream and watch it collapse
//! into a handful of templates. Reports ingest throughput and compression ratio.
//!
//! Run (use --release for meaningful timing):
//!     cargo run --release -p logdrain --example highvolume            # 1,000,000 lines
//!     cargo run --release -p logdrain --example highvolume -- 5000000 # custom count

use std::time::Instant;

use logdrain::{builtin_masks, Miner};

/// Tiny deterministic xorshift64 PRNG (no external deps, reproducible output).
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

fn uuid(rng: &mut Rng) -> String {
    let (a, b, c) = (rng.next_u64(), rng.next_u64(), rng.next_u64());
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        a as u32,
        (a >> 32) as u16,
        b as u16,
        (b >> 16) as u16,
        c & 0xffff_ffff_ffff,
    )
}

fn ipv4(rng: &mut Rng) -> String {
    format!(
        "{}.{}.{}.{}",
        rng.below(256),
        rng.below(256),
        rng.below(256),
        rng.below(256)
    )
}

/// Build one realistic log line. Distinct *shapes* are few; the variable fields
/// (ids, uuids, ips, latencies, statuses) are high-cardinality noise.
fn gen_line(rng: &mut Rng) -> String {
    let status = [200u32, 200, 200, 404, 500, 503][rng.below(6) as usize];
    let lat = rng.below(500) + 1;
    let id = rng.below(100_000);
    let domains = ["example.com", "corp.io", "mail.net", "acme.org"];
    let domain = domains[rng.below(domains.len() as u64) as usize];
    match rng.below(8) {
        0 => format!("GET /api/v1/servers/{id}/metrics {status} {lat}ms"),
        1 => format!("POST /api/v1/users/{id}/login {status} from {}", ipv4(rng)),
        2 => format!("request {} completed in {lat}ms", uuid(rng)),
        3 => format!("signup for user{id}@{domain} from {}", ipv4(rng)),
        4 => format!("cache miss for key user:{id}:profile"),
        5 => format!("WARN slow query {lat}ms on table orders"),
        6 => format!("DELETE /api/v1/sessions/{id} {status}"),
        _ => format!("payment {} for order {id} status {status}", uuid(rng)),
    }
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(1_000_000);

    // Pre-generate the corpus so timing measures the miner, not string building.
    let mut rng = Rng(0x9E3779B97F4A7C15);
    print!("generating {n} lines... ");
    let lines: Vec<String> = (0..n).map(|_| gen_line(&mut rng)).collect();
    println!("done");

    let miner = Miner::builder()
        .path_delimiters(&['/'])
        .masks([
            builtin_masks::uuid(),
            builtin_masks::ipv4(),
            builtin_masks::email(),
        ])
        .build()
        .unwrap();

    let start = Instant::now();
    for line in &lines {
        miner.add(line);
    }
    let elapsed = start.elapsed();

    let per_line = elapsed.as_nanos() as f64 / n as f64;
    let per_sec = n as f64 / elapsed.as_secs_f64();
    let clusters = miner.len();

    println!();
    println!("ingested : {n} lines in {elapsed:.2?}");
    println!("speed    : {per_sec:.0} lines/sec  ({per_line:.0} ns/line)");
    println!(
        "templates: {clusters}  ({:.0}x compression)",
        n as f64 / clusters as f64
    );

    println!("\ntop templates by volume:");
    let mut cs = miner.clusters();
    cs.sort_by_key(|b| std::cmp::Reverse(b.size()));
    println!("{:>9}  TEMPLATE", "SIZE");
    println!("{}", "-".repeat(60));
    for c in cs.iter().take(10) {
        println!("{:>9}  {}", c.size(), c.template());
    }
}
