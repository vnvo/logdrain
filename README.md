# logdrain

[![CI](https://github.com/vnvo/logdrain/actions/workflows/ci.yml/badge.svg)](https://github.com/vnvo/logdrain/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/logdrain.svg)](https://crates.io/crates/logdrain)
[![docs.rs](https://img.shields.io/docsrs/logdrain)](https://docs.rs/logdrain)
[![License](https://img.shields.io/badge/license-Apache--2.0%20OR%20MIT-blue.svg)](#msrv--license)
[![MSRV](https://img.shields.io/badge/rustc-1.80%2B-orange.svg)](#msrv--license)

High-throughput, online log-template mining in Rust - the `Drain3` algorithm with
path-preserving tokenization, configurable masks, and stack-trace clustering.

`logdrain` turns a stream of noisy log lines into a small set of templates. Feed it
lines; it returns a cluster id per line and learns templates incrementally - no
batch training step, no model files.

```text
GET /api/v1/servers/409/metrics 200 12ms   ┐
GET /api/v1/servers/410/metrics 200  9ms   ├─>  GET /api/v1/servers/<*>/metrics <*> <*>
GET /api/v1/servers/873/metrics 503 41ms   ┘
request 550e8400-…-446655440000 done 88ms  ┐
request 6ba7b810-…-00c04fd430c8 done 51ms  ├─>  request <uuid> done <*>
request 7c9e6679-…-e07fc1f90ae7 done 75ms  ┘
```

## Status

Two crates are built and tested:

- **`logdrain`** : the core library (online miner, tokenizer, masks, persistence).
- **`logdrain-cli`** : the `logdrain` command-line tool.

An HTTP service (`draind`) and out-of-tree persistence backends (Redis, S3) are on
the roadmap, not yet implemented.

## Features

Beyond the core Drain algorithm, logdrain adds the things you actually need to run
template mining in production:

- **Online & incremental** - `add(line)` returns immediately with a cluster id and
  whether the template was created, generalized, or matched unchanged. No batch
  training step, no model files.
- **Path-preserving tokenization** - `/servers/409/foo` clusters to
  `/servers/<*>/foo`, not `<*>`; delimiters are retained through generalization.
  Keeps API/URL/structured logs readable instead of collapsing them to noise.
- **Configurable masks** - a regex pre-pass replaces high-cardinality tokens with
  named placeholders before clustering. Built-ins: `uuid`, `hex32`, `email`,
  `ipv4`, `jwt`; custom masks via `Mask::new(pattern, placeholder)`.
- **Stack-trace clustering** - `first_line_only` mode clusters multi-line traces on
  their first line and keeps the rest of each trace verbatim as a per-cluster suffix.
- **Numeric parametrization** - pure-number tokens generalize to the wildcard.
- **Parameter extraction** - `extract()` pulls the variable values back out of a
  matched line.
- **Members** - attach and de-duplicate labels (host, service, …) per cluster.
- **Exhaustive & persistent** - every template is kept with an exact count (no
  sampling, no top-N truncation); sync `snapshot`/`restore` with a pluggable backend
  (Memory + File in core) keeps that catalog for as long as you keep the state.
- **Thread-safe & concurrent** - `add(&self)` from many threads; the tree shards by
  token count and the match path runs under a shared read lock.
- **Streaming memory** - lines are never retained; footprint scales with the number
  of distinct templates (bounded by a per-leaf LRU), not with lines ingested.
- **Embeddable & dependency-light** - a small Rust library you drop into your own
  pipeline. No service to run, no vendor, no storage assumptions.

## Library quick start

```sh
cargo add logdrain
```

```rust
use logdrain::{builtin_masks, Miner};

let miner = Miner::builder()
    .path_delimiters(&['/'])
    .masks([builtin_masks::uuid(), builtin_masks::ipv4()])
    .sim_threshold(0.4)
    .build()
    .unwrap();

miner.add("GET /api/v1/servers/409/metrics from 10.0.0.1");
miner.add("GET /api/v1/servers/410/metrics from 10.0.0.2");

for c in miner.clusters() {
    println!("#{} x{}  {}", c.id(), c.size(), c.template());
    // => #1 x2  GET /api/v1/servers/<*>/metrics from <ipv4>
}

// Classify a new line and pull out the variable parts.
let (id, params) = miner.extract("GET /api/v1/servers/777/metrics from 10.0.0.9").unwrap();
assert_eq!(params, ["777"]); // <ipv4> is a named placeholder, not a wildcard
```

### Persistence

```rust
use logdrain::{FilePersistence, Miner};

let store = FilePersistence::new("templates.bin");
miner.save_state(&store).unwrap();        // atomic write (tmp + fsync + rename)

let restored = Miner::builder().build().unwrap();
restored.load_state(&store).unwrap();     // rebuilds the tree from the snapshot
```

`Persistence` is a small sync trait; `MemoryPersistence` and `FilePersistence` ship
in core, and `Miner::{snapshot, restore}` expose the raw bytes if you want to store
them elsewhere.

## CLI

```sh
cargo install logdrain-cli          # installs the `logdrain` binary on $PATH
# or, from this repo:
cargo run -p logdrain-cli -- [OPTIONS] [FILES]...
```

Reads the given files, or stdin if none. One log record per line.

```sh
# Cluster an access log, preserving paths and masking ids
logdrain --path-delimiters / --masks uuid,ipv4,email access.log

# JSON output, largest clusters first
cat app.log | logdrain --format json --sort size

# Pull the message out of JSON lines first
cat events.ndjson | logdrain --key event.message
```

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

## Examples

```sh
cargo run -p logdrain --example basic        # masks, paths, params on readable data
cargo run -p logdrain --example stacktrace   # multi-line traces -> ranked failures
cargo run --release -p logdrain --example highvolume -- 5000000   # single-thread throughput
cargo run --release -p logdrain --example scaling                 # multi-core scaling table
```

## Performance

Tested numbers from one mid-range Linux box — they **vary with hardware and load**,
so treat them as orders of magnitude and re-run on your own target.

- **Latency** (criterion, `cargo bench -p logdrain --bench add`): steady `add` ~0.3 µs,
  cold-start `add` ~1.8 µs, `match_only` ~0.25 µs.
- **Throughput** (bundled examples, single timed run): **~1–2M lines/sec single-thread**,
  scaling to **multi-M lines/sec across cores** (~3.3× on 8). Sub-linear; the ceiling
  is contention on hot shared template counters.
- **Memory** is bounded by template count, not input: streaming a file through the CLI
  stays in single-digit MB regardless of line count.

## MSRV & license

Rust 1.80+. Licensed under either of Apache-2.0 or MIT, at your option.
