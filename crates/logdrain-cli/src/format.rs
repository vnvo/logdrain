//! Rendering clusters as text, JSON, JSON Lines (NDJSON), or CSV.

use std::time::{SystemTime, UNIX_EPOCH};

use clap::ValueEnum;
use logdrain::Cluster;

/// Output format selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Aligned, human-readable columns.
    Text,
    /// JSON array of cluster objects (pretty-printed).
    Json,
    /// JSON Lines: one compact JSON object per line, for streaming into other
    /// systems (jq, Vector, Logstash, Kafka, Elasticsearch, ...).
    #[value(alias = "ndjson")]
    Jsonl,
    /// RFC-4180 CSV with a header row.
    Csv,
}

/// Cluster ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Sort {
    /// Largest clusters first.
    Size,
    /// Alphabetical by template.
    Template,
    /// Ascending by cluster id.
    Id,
    /// Highest lines-per-minute first.
    Rate,
}

fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Filter by minimum size and order clusters per `sort`.
pub fn prepare(mut clusters: Vec<Cluster>, min_size: u64, sort: Sort) -> Vec<Cluster> {
    clusters.retain(|c| c.size() >= min_size);
    match sort {
        Sort::Size => clusters.sort_by(|a, b| b.size().cmp(&a.size()).then(a.id().cmp(&b.id()))),
        Sort::Template => clusters.sort_by(|a, b| a.template().cmp(b.template())),
        Sort::Id => clusters.sort_by_key(|a| a.id()),
        Sort::Rate => clusters.sort_by(|a, b| {
            b.lines_per_minute()
                .partial_cmp(&a.lines_per_minute())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.id().cmp(&b.id()))
        }),
    }
    clusters
}

/// Render already-prepared clusters in the requested format.
pub fn render(clusters: &[Cluster], format: Format) -> String {
    match format {
        Format::Text => render_text(clusters),
        Format::Json => render_json(clusters),
        Format::Jsonl => render_jsonl(clusters),
        Format::Csv => render_csv(clusters),
    }
}

fn render_text(clusters: &[Cluster]) -> String {
    let show_rate = clusters.iter().any(|c| c.lines_per_minute() > 0.0);
    let id_w = clusters
        .iter()
        .map(|c| c.id().to_string().len())
        .max()
        .unwrap_or(2)
        .max(2);
    let size_w = clusters
        .iter()
        .map(|c| c.size().to_string().len())
        .max()
        .unwrap_or(4)
        .max(4);

    let mut out = String::new();
    if show_rate {
        out.push_str(&format!(
            "{:>id_w$}  {:>size_w$}  {:>9}  TEMPLATE\n",
            "ID", "SIZE", "RATE/MIN"
        ));
    } else {
        out.push_str(&format!("{:>id_w$}  {:>size_w$}  TEMPLATE\n", "ID", "SIZE"));
    }
    for c in clusters {
        if show_rate {
            out.push_str(&format!(
                "{:>id_w$}  {:>size_w$}  {:>9.1}  {}\n",
                c.id(),
                c.size(),
                c.lines_per_minute(),
                c.template()
            ));
        } else {
            out.push_str(&format!(
                "{:>id_w$}  {:>size_w$}  {}\n",
                c.id(),
                c.size(),
                c.template()
            ));
        }
    }
    out
}

/// One cluster as a JSON object. Shared by the `json` and `jsonl` renderers so the
/// two formats never drift apart.
fn cluster_json(c: &Cluster) -> serde_json::Value {
    serde_json::json!({
        "id": c.id(),
        "size": c.size(),
        "template": c.template(),
        "linesPerMinute": c.lines_per_minute(),
        "createdAt": unix_ms(c.created_at()),
        "updatedAt": unix_ms(c.updated_at()),
        // Event-time fields are null unless --time-key supplied timestamps.
        "eventFirstSeen": c.event_first_seen().map(unix_ms),
        "eventLastSeen": c.event_last_seen().map(unix_ms),
        "eventRatePerMinute": c.event_lines_per_minute(),
        "members": c.members().iter().map(|m| m.to_string()).collect::<Vec<_>>(),
        "suffix": c.suffix(),
    })
}

fn render_json(clusters: &[Cluster]) -> String {
    let arr: Vec<serde_json::Value> = clusters.iter().map(cluster_json).collect();
    serde_json::to_string_pretty(&arr).expect("cluster json is always serializable")
}

fn render_jsonl(clusters: &[Cluster]) -> String {
    let mut out = String::new();
    for c in clusters {
        let obj =
            serde_json::to_string(&cluster_json(c)).expect("cluster json is always serializable");
        out.push_str(&obj);
        out.push('\n');
    }
    out
}

fn csv_escape(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn render_csv(clusters: &[Cluster]) -> String {
    let mut out = String::from(
        "id,size,rate_per_min,template,created_at,updated_at,\
         event_first_seen,event_last_seen,event_rate_per_min,members,suffix\n",
    );
    for c in clusters {
        let opt_ms = |t: Option<std::time::SystemTime>| t.map(unix_ms).map(|n| n.to_string());
        let row = [
            c.id().to_string(),
            c.size().to_string(),
            format!("{:.1}", c.lines_per_minute()),
            c.template().to_string(),
            unix_ms(c.created_at()).to_string(),
            unix_ms(c.updated_at()).to_string(),
            opt_ms(c.event_first_seen()).unwrap_or_default(),
            opt_ms(c.event_last_seen()).unwrap_or_default(),
            c.event_lines_per_minute()
                .map(|r| format!("{r:.1}"))
                .unwrap_or_default(),
            c.members()
                .iter()
                .map(|m| m.to_string())
                .collect::<Vec<_>>()
                .join(";"),
            c.suffix().unwrap_or("").to_string(),
        ];
        let escaped: Vec<String> = row.iter().map(|f| csv_escape(f)).collect();
        out.push_str(&escaped.join(","));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use logdrain::Miner;

    fn sample() -> Vec<Cluster> {
        let m = Miner::builder().build().unwrap();
        m.add("user 1 logged in"); // cluster 1
        m.add("user 2 logged in"); // joins 1
        m.add("disk full warning now"); // cluster 2
        m.clusters()
    }

    #[test]
    fn prepare_sorts_by_size_then_id() {
        let cs = prepare(sample(), 0, Sort::Size);
        assert_eq!(cs[0].size(), 2); // "user ... logged in" cluster first
        assert_eq!(cs[1].size(), 1);
    }

    #[test]
    fn prepare_filters_min_size() {
        let cs = prepare(sample(), 2, Sort::Size);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].size(), 2);
    }

    #[test]
    fn prepare_sort_by_id() {
        let cs = prepare(sample(), 0, Sort::Id);
        assert!(cs[0].id() < cs[1].id());
    }

    #[test]
    fn text_has_header_and_no_rate_column_for_instant_clusters() {
        let out = render(&prepare(sample(), 0, Sort::Size), Format::Text);
        assert!(out.contains("TEMPLATE"));
        assert!(!out.contains("RATE/MIN")); // instant clusters -> rate 0 -> column hidden
        assert!(out.contains("user <*> logged in"));
    }

    #[test]
    fn json_is_array_of_objects() {
        let out = render(&prepare(sample(), 0, Sort::Size), Format::Json);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v.is_array());
        assert_eq!(v[0]["template"], "user <*> logged in");
        assert_eq!(v[0]["size"], 2);
        assert!(v[0]["linesPerMinute"].is_number());
    }

    #[test]
    fn jsonl_is_one_object_per_line() {
        let cs = prepare(sample(), 0, Sort::Size);
        let out = render(&cs, Format::Jsonl);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), cs.len()); // one line per cluster, no array wrapper
        for line in &lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(v.is_object());
            assert!(!line.contains('\n'));
        }
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(lines[0]).unwrap()["template"],
            "user <*> logged in"
        );
    }

    #[test]
    fn csv_has_header_and_rows() {
        let out = render(&prepare(sample(), 0, Sort::Size), Format::Csv);
        let mut lines = out.lines();
        assert_eq!(
            lines.next().unwrap(),
            "id,size,rate_per_min,template,created_at,updated_at,\
             event_first_seen,event_last_seen,event_rate_per_min,members,suffix"
        );
        assert!(lines.next().unwrap().contains("user <*> logged in"));
    }

    #[test]
    fn event_fields_are_null_without_event_time_and_set_with_it() {
        // No event time -> JSON event fields are null.
        let plain = render(&prepare(sample(), 0, Sort::Size), Format::Jsonl);
        let first: serde_json::Value = serde_json::from_str(plain.lines().next().unwrap()).unwrap();
        assert!(first["eventFirstSeen"].is_null());
        assert!(first["eventRatePerMinute"].is_null());

        // With event time -> populated.
        let m = Miner::builder().build().unwrap();
        m.add_at("user 1 in", 60_000);
        m.add_at("user 2 in", 180_000); // span 120s
        let out = render(&prepare(m.clusters(), 0, Sort::Size), Format::Jsonl);
        let v: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
        assert_eq!(v["eventFirstSeen"], 60_000);
        assert_eq!(v["eventLastSeen"], 180_000);
        assert_eq!(v["eventRatePerMinute"], 1.0); // 2 lines / 2 min
    }

    #[test]
    fn csv_escapes_special_characters() {
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_escape("line1\nline2"), "\"line1\nline2\"");
    }
}
