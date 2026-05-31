//! Minimal end-to-end usage of the logdrain miner.
//! Run with: `cargo run -p logdrain --example basic`

use logdrain::{Miner, UpdateType};

fn main() {
    let miner = Miner::builder()
        .sim_threshold(0.4)
        .depth(4)
        .build()
        .unwrap();

    let lines = [
        "user 42 logged in from 10.0.0.1",
        "user 99 logged in from 10.0.0.2",
        "user 7 logged in from 10.0.0.3",
        "disk usage at 80 percent",
        "disk usage at 95 percent",
    ];

    for line in lines {
        let r = miner.add(line);
        let tag = match r.update {
            UpdateType::Created => "NEW",
            UpdateType::TemplateChanged => "GEN",
            UpdateType::None => "HIT",
        };
        println!("[{tag}] #{} <- {line}", r.cluster_id);
    }

    println!("\n{} clusters:", miner.len());
    let mut clusters = miner.clusters();
    clusters.sort_by_key(|c| c.id());
    for c in clusters {
        println!("  #{} (size {}): {}", c.id(), c.size(), c.template());
    }
}
