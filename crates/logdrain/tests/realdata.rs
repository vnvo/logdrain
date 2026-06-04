//! Golden test over a representative log sample: assert the mined cluster count,
//! total coverage, and a few expected templates stay stable.

use logdrain::{builtin_masks, Miner};

const SAMPLE: &str = include_str!("../testdata/sample.log");

fn mine_sample() -> Miner {
    let miner = Miner::builder()
        .path_delimiters(&['/'])
        .masks([
            builtin_masks::uuid(),
            builtin_masks::ipv4(),
            builtin_masks::email(),
        ])
        .build()
        .unwrap();
    for line in SAMPLE.lines().filter(|l| !l.trim().is_empty()) {
        miner.add(line);
    }
    miner
}

#[test]
fn sample_clusters_to_expected_count_and_compression() {
    let lines = SAMPLE.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(lines, 30, "fixture size changed");

    let miner = mine_sample();
    assert_eq!(miner.len(), 9, "cluster count drifted");

    // Every line landed in some cluster (sizes sum to the input count).
    let total: u64 = miner.clusters().iter().map(|c| c.size()).sum();
    assert_eq!(total, 30);

    let ratio = lines as f64 / miner.len() as f64;
    assert!(ratio >= 3.0, "compression {ratio:.2}x below 3x");
}

#[test]
fn sample_produces_expected_templates() {
    let miner = mine_sample();
    let templates: Vec<String> = miner
        .clusters()
        .iter()
        .map(|c| c.template().to_string())
        .collect();

    for expected in [
        "GET /api/v1/servers/<*>/metrics <*> <*>",
        "POST /api/v1/users/<*>/login <*> from <ipv4>",
        "request <uuid> completed in <*>",
        "signup for <email> from <ipv4>",
        "cache miss for key <*>",
    ] {
        assert!(
            templates.iter().any(|t| t == expected),
            "missing template: {expected}\ngot: {templates:#?}"
        );
    }
}
