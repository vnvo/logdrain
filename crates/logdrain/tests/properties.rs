//! Property tests for v0.1 invariants.

use logdrain::{Miner, MinerBuilder};
use proptest::prelude::*;

fn miner() -> Miner {
    Miner::from_options(MinerBuilder::new().build_options().unwrap())
}

proptest! {
    // Adding the exact same line twice never changes the template on the 2nd add.
    #[test]
    fn generalization_idempotent(words in prop::collection::vec("[a-z]{1,6}", 1..8)) {
        let line = words.join(" ");
        let m = miner();
        let a = m.add(&line);
        let tmpl_a = m.cluster(a.cluster_id).unwrap().template().to_string();
        let b = m.add(&line);
        prop_assert_eq!(a.cluster_id, b.cluster_id);
        let tmpl_b = m.cluster(b.cluster_id).unwrap().template().to_string();
        prop_assert_eq!(tmpl_a, tmpl_b);
    }

    // match_only after add always finds the same cluster the line was assigned to.
    #[test]
    fn add_then_match_is_consistent(words in prop::collection::vec("[a-z]{1,6}", 1..8)) {
        let line = words.join(" ");
        let m = miner();
        let a = m.add(&line);
        prop_assert_eq!(m.match_only(&line), Some(a.cluster_id));
    }

    // Snapshot round-trip preserves cluster count and per-id templates.
    #[test]
    fn snapshot_round_trip(lines in prop::collection::vec(
        prop::collection::vec("[a-z]{1,5}", 1..6).prop_map(|w| w.join(" ")),
        1..30,
    )) {
        let m = miner();
        for l in &lines { m.add(l); }
        let bytes = m.snapshot();
        let m2 = miner();
        m2.restore(&bytes).unwrap();
        prop_assert_eq!(m.len(), m2.len());
        for c in m.clusters() {
            let restored = m2.cluster(c.id()).expect("cluster id present after restore");
            prop_assert_eq!(c.template(), restored.template());
        }
    }
}

fn path_miner() -> Miner {
    Miner::from_options(
        MinerBuilder::new()
            .path_delimiters(&['/'])
            .build_options()
            .unwrap(),
    )
}

fn first_line_miner() -> Miner {
    Miner::from_options(
        MinerBuilder::new()
            .first_line_only(true)
            .build_options()
            .unwrap(),
    )
}

proptest! {
    // With path delimiters, add-then-match is consistent for a path line.
    #[test]
    fn path_add_then_match(segs in prop::collection::vec("[a-z]{1,5}", 1..6)) {
        let line = format!("/{}", segs.join("/"));
        let m = path_miner();
        let a = m.add(&line);
        prop_assert_eq!(m.match_only(&line), Some(a.cluster_id));
    }

    // The rendered path template re-tokenizes to the same number of tokens
    // (delimiter-aware rendering is structurally faithful).
    #[test]
    fn path_template_token_count_stable(segs in prop::collection::vec("[a-z]{1,5}", 1..6)) {
        let line = format!("/{}", segs.join("/"));
        let m = path_miner();
        let a = m.add(&line);
        let template = m.cluster(a.cluster_id).unwrap().template().to_string();
        // Re-feeding the rendered template lands in the same cluster.
        prop_assert_eq!(m.match_only(&template), Some(a.cluster_id));
    }

    // In first-line-only mode, the suffix captured at creation is stable when the
    // same first line is re-added with a different suffix.
    #[test]
    fn suffix_is_stable(
        first in "[a-z][a-z ]{0,18}",
        suffix in "[a-zA-Z0-9 \n]{0,20}",
    ) {
        let m = first_line_miner();
        let a = m.add(&format!("{first}\n{suffix}"));
        let captured = m.cluster(a.cluster_id).unwrap().suffix().map(|s| s.to_string());
        let b = m.add(&format!("{first}\nDIFFERENT SUFFIX"));
        prop_assert_eq!(a.cluster_id, b.cluster_id);
        prop_assert_eq!(
            m.cluster(a.cluster_id).unwrap().suffix().map(|s| s.to_string()),
            captured
        );
    }
}
