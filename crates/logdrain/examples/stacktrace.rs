//! Cluster multi-line stack traces by their first line (path-aware), keeping the
//! full trace of the first occurrence as a verbatim suffix. This is how you turn
//! a flood of near-identical exceptions into a ranked list of distinct failures.
//! Run with: `cargo run -p logdrain --example stacktrace`

use logdrain::Miner;

fn main() {
    let miner = Miner::builder()
        .first_line_only(true)
        .path_delimiters(&['/'])
        .build()
        .unwrap();

    // Six traces, three distinct failures. The frames differ line-to-line, but
    // clustering keys on the first line so each failure mode collapses to one.
    let traces = [
        "ERROR NullPointerException at com/acme/svc/OrderHandler.process\n\tat OrderHandler.process(OrderHandler.java:142)\n\tat Dispatcher.run(Dispatcher.java:88)\n\tat java.base/Thread.run(Thread.java:829)",
        "ERROR NullPointerException at com/acme/svc/OrderHandler.process\n\tat OrderHandler.process(OrderHandler.java:155)\n\tat Dispatcher.run(Dispatcher.java:88)",
        "ERROR NullPointerException at com/acme/svc/OrderHandler.process\n\tat OrderHandler.process(OrderHandler.java:142)\n\tat Retry.attempt(Retry.java:20)",
        "ERROR SQLTimeoutException at com/acme/db/ConnectionPool.acquire\n\tat ConnectionPool.acquire(ConnectionPool.java:64)\n\tat OrderHandler.load(OrderHandler.java:71)",
        "ERROR SQLTimeoutException at com/acme/db/ConnectionPool.acquire\n\tat ConnectionPool.acquire(ConnectionPool.java:64)\n\tat Report.build(Report.java:31)",
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
