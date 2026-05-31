//! Stack-trace clustering: cluster on the first line (path-aware), keep the rest
//! of each trace verbatim as a suffix. Run with:
//! `cargo run -p logdrain --example stacktrace`

use logdrain::Miner;

fn main() {
    let miner = Miner::builder()
        .first_line_only(true)
        .path_delimiters(&['/'])
        .build()
        .unwrap();

    let traces = [
        "NullPointerException in /app/svc/Handler.java\n  at Handler.process(Handler.java:42)\n  at Server.run(Server.java:88)",
        "NullPointerException in /app/svc/Handler.java\n  at Handler.process(Handler.java:51)",
        "TimeoutException in /app/svc/Client.java\n  at Client.call(Client.java:12)",
    ];

    for t in traces {
        let r = miner.add(t);
        println!(
            "#{} ({:?}) <- {}",
            r.cluster_id,
            r.update,
            t.lines().next().unwrap()
        );
    }

    println!("\n{} clusters:", miner.len());
    let mut clusters = miner.clusters();
    clusters.sort_by_key(|c| c.id());
    for c in clusters {
        println!("  #{} (size {}): {}", c.id(), c.size(), c.template());
        if let Some(suffix) = c.suffix() {
            println!("      suffix (from first occurrence):");
            for line in suffix.lines() {
                println!("        {line}");
            }
        }
    }
}
