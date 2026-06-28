//! Cluster multi-line stack traces by their first line, keeping the rest of each
//! trace as a per-cluster suffix. Masks turn per-request ids / client IPs in the
//! first line into placeholders, so the same failure collapses to one template
//! regardless of those high-cardinality values.
//! Run with: `cargo run -p logdrain --example stacktrace`

use logdrain::{builtin_masks, Miner};

fn main() {
    let miner = Miner::builder()
        .first_line_only(true)
        .path_delimiters(&['/'])
        .masks([builtin_masks::uuid(), builtin_masks::ipv4()])
        .build()
        .unwrap();

    // Six traces, three distinct failures. The frames differ, and the first lines
    // carry per-request ids / client IPs - masking turns those into placeholders so
    // each failure mode collapses to a single template.
    let traces = [
        "ERROR NullPointerException req 550e8400-e29b-41d4-a716-446655440000 at com/acme/svc/OrderHandler.process\n\tat OrderHandler.process(OrderHandler.java:142)\n\tat Dispatcher.run(Dispatcher.java:88)\n\tat java.base/Thread.run(Thread.java:829)",
        "ERROR NullPointerException req 6ba7b810-9dad-11d1-80b4-00c04fd430c8 at com/acme/svc/OrderHandler.process\n\tat OrderHandler.process(OrderHandler.java:155)\n\tat Dispatcher.run(Dispatcher.java:88)",
        "ERROR NullPointerException req 7c9e6679-7425-40de-944b-e07fc1f90ae7 at com/acme/svc/OrderHandler.process\n\tat OrderHandler.process(OrderHandler.java:142)\n\tat Retry.attempt(Retry.java:20)",
        "ERROR SQLTimeoutException from 10.0.0.5 at com/acme/db/ConnectionPool.acquire\n\tat ConnectionPool.acquire(ConnectionPool.java:64)\n\tat OrderHandler.load(OrderHandler.java:71)",
        "ERROR SQLTimeoutException from 10.14.2.3 at com/acme/db/ConnectionPool.acquire\n\tat ConnectionPool.acquire(ConnectionPool.java:64)\n\tat Report.build(Report.java:31)",
        "WARN RetryableException at com/acme/net/HttpClient.call\n\tat HttpClient.call(HttpClient.java:33)",
    ];

    for t in traces {
        miner.add(t);
    }

    let mut clusters = miner.clusters();
    clusters.sort_by(|a, b| b.size().cmp(&a.size()).then(a.id().cmp(&b.id())));

    println!(
        "{} traces  ->  {} distinct failures\n",
        clusters.iter().map(|c| c.size()).sum::<u64>(),
        clusters.len(),
    );

    for c in &clusters {
        println!("#{}  x{}  {}", c.id(), c.size(), c.template());
        if let Some(suffix) = c.suffix() {
            for frame in suffix.lines() {
                println!("        {}", frame.trim_start());
            }
        }
        println!();
    }
}
