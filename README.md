<div align="center">

# logdrain

**High-throughput, online log-template mining in Rust.**

The Drain3 algorithm with path-preserving tokenization, masks, and stack-trace
clustering - as an embeddable library and a CLI.

[![CI](https://github.com/vnvo/logdrain/actions/workflows/ci.yml/badge.svg)](https://github.com/vnvo/logdrain/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/logdrain.svg)](https://crates.io/crates/logdrain)
[![docs.rs](https://img.shields.io/docsrs/logdrain)](https://docs.rs/logdrain)
[![License](https://img.shields.io/badge/license-Apache--2.0%20OR%20MIT-blue.svg)](#license)
[![MSRV](https://img.shields.io/badge/rustc-1.85%2B-orange.svg)](#license)

</div>

---

Feed `logdrain` a stream of noisy log lines and it learns their **templates**
incrementally - returning a cluster id per line, with no batch training and no model
files.

```text
GET /api/v1/servers/409/metrics 200 12ms   ┐
GET /api/v1/servers/410/metrics 200  9ms   ├─►  GET /api/v1/servers/<*>/metrics <*> <*>
GET /api/v1/servers/873/metrics 503 41ms   ┘
request 550e8400-…-446655440000 done 88ms  ┐
request 6ba7b810-…-00c04fd430c8 done 51ms  ├─►  request <uuid> done <*>
request 7c9e6679-…-e07fc1f90ae7 done 75ms  ┘
```

> **5,000,000 noisy lines → 8 templates in ~2 s on a single core.** Memory tracks
> the number of templates, not the volume of input.

## ✨ Features

#### Online & incremental
`add(line)` returns a cluster id instantly - *created*, *generalized*, or *matched*. No batch step.

#### Path-preserving tokenization
`/servers/409/foo` clusters to `/servers/<*>/foo`, not `<*>`. URLs and structured logs stay readable.

#### Configurable masks
UUIDs, IPs, emails, JWTs → named placeholders before clustering - built-in or custom.

#### Stack-trace clustering
Cluster multi-line traces on their first line and keep the full trace as a per-cluster suffix.

#### Extraction & members
`extract()` recovers the variable values of a line; attach de-duplicated labels per cluster.

#### Exhaustive & persistent
Exact per-template counts (no sampling, no top-N) with `snapshot`/`restore` to a pluggable backend.

#### Thread-safe & fast
`add(&self)` from many threads; a sharded, mostly-lock-free hot path (~1-2M lines/sec per core).

## 💡 Use cases

#### Noise reduction & triage
Collapse millions of lines into a handful of templates, ranked by volume, to see what's *actually* happening.

#### New-pattern alerting
Alert on the `Created` signal - the first time a never-seen log shape appears (new errors, attacks, regressions).

#### Incident & exception clustering
Rank distinct stack-trace failures by frequency, so you triage the top one instead of scrolling.

#### Pipeline cost reduction
Store `(template, count)` instead of raw lines, or sample by template - a Vector/Cribl-style stage.

#### Structured extraction
Turn unstructured logs into `(template_id, params)` for dashboards, ML features, or alerting.

#### Edge / agent summarization
Embed it in a log shipper to summarize at the source and cut egress and ingest cost.

## 📦 Install

```sh
cargo add logdrain                  # the library
cargo install logdrain-cli          # the `logdrain` CLI, on your $PATH
```

## 🖥️  CLI

Reads files (or stdin), one record per line, and prints the templates it found:

```console
$ logdrain --path-delimiters / --masks uuid,ipv4,email access.log
ID  SIZE  TEMPLATE
 1     5  GET /api/v1/servers/<*>/metrics <*> <*>
 2     4  POST /api/v1/users/<*>/login <*> from <ipv4>
 3     4  request <uuid> completed in <*>
 4     4  signup for <email> from <ipv4>
 5     4  cache miss for key <*>
 …          (30 lines → 9 templates)
```

```sh
cat app.log | logdrain --format json --sort size   # JSON, biggest clusters first
cat events.ndjson | logdrain --key event.message    # pull a field out of JSON lines
```

<details>
<summary><b>All CLI options</b></summary>

| Option | Description |
|---|---|
| `--format text\|json\|csv` | output format (default `text`) |
| `--key <FIELD>` | parse each line as JSON and extract this dot-path field |
| `--masks <names>` | comma list of built-ins: `uuid,hex32,email,ipv4,jwt` |
| `--path-delimiters <CHARS>` | characters preserved as token boundaries, e.g. `/` |
| `--first-line-only` | cluster on the first line; keep the rest as a suffix |
| `--sort size\|template\|id\|rate` | ordering (default `size`) |
| `--min-size <N>` | hide clusters smaller than N |
| `--sim-th <FLOAT>` | similarity threshold (default `0.4`) |
| `--depth <N>` | prefix-tree depth (default `4`) |

</details>

## 📚 Library

```rust
use logdrain::{builtin_masks, Miner};

let miner = Miner::builder()
    .path_delimiters(&['/'])
    .masks([builtin_masks::uuid(), builtin_masks::ipv4()])
    .build()
    .unwrap();

miner.add("GET /api/v1/servers/409/metrics from 10.0.0.1");
miner.add("GET /api/v1/servers/410/metrics from 10.0.0.2");

for c in miner.clusters() {
    println!("#{} x{}  {}", c.id(), c.size(), c.template());
    // => #1 x2  GET /api/v1/servers/<*>/metrics from <ipv4>
}

// Classify a new line and recover its variable parts.
let (_id, params) = miner.extract("GET /api/v1/servers/777/metrics from 10.0.0.9").unwrap();
assert_eq!(params, ["777"]); // <ipv4> is a named placeholder, not a wildcard
```

<details>
<summary><b>Persisting state</b></summary>

```rust
use logdrain::{FilePersistence, Miner};

let store = FilePersistence::new("templates.bin");
miner.save_state(&store).unwrap();        // atomic write (tmp + fsync + rename)

let restored = Miner::builder().build().unwrap();
restored.load_state(&store).unwrap();     // rebuilds the tree from the snapshot
```

`Persistence` is a small sync trait; `MemoryPersistence` and `FilePersistence` ship in
core, and `Miner::{snapshot, restore}` expose the raw bytes for any other backend.

</details>

## ⚡ Performance

Numbers from one mid-range Linux box - they **vary with hardware and load**, so treat
them as orders of magnitude and re-run on your own target.

| | |
|---|---|
| Throughput (single thread) | **~1-2M lines/sec** |
| Throughput (8 threads) | multi-M lines/sec (~3.3× scaling) |
| `add` latency (steady / cold) | ~0.3 µs / ~1.8 µs |
| Memory | bounded by template count, not input |

Reproduce: `cargo bench -p logdrain --bench add`, or
`cargo run --release -p logdrain --example scaling`.

## 🗺️  Status & roadmap

**Available:** `logdrain` (library) and `logdrain-cli`, published and tested.
**Roadmap:** an HTTP service (`draind`), Redis/S3 persistence backends, and
hierarchical/distributed aggregation.

## 🤝 Contributing

Issues and pull requests welcome. Please keep the standard checks green:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

## ⚖️  License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your
option. Minimum supported Rust version: **1.85**.
