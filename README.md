# logdrain

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

- **Online & incremental** - `add(line)` returns immediately with a cluster id and
  whether the template was created, generalized, or matched unchanged.
- **Path-preserving tokenization** - `/servers/409/foo` clusters to
  `/servers/<*>/foo`, not `<*>`; delimiters are retained through generalization.
- **Masks** - a regex pre-pass replaces high-cardinality tokens with named
  placeholders before clustering. Built-ins: `uuid`, `hex32`, `email`, `ipv4`,
  `jwt`; custom masks are a `Mask::new(pattern, placeholder)` away.
- **Stack-trace clustering** - `first_line_only` mode clusters on the first line and
  keeps the rest of each trace verbatim as a per-cluster suffix.
- **Numeric parametrization, members, parameter extraction, snapshot/restore.**
- **Streaming memory** - lines are never retained; footprint scales with the number
  of distinct templates (bounded by a per-leaf LRU), not with lines ingested.

## Library quick start

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
cargo install logdrain-cli          # installs the `logdrain` binary
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
cargo run --release -p logdrain --example highvolume -- 5000000   # scale + throughput
```

## Performance

Single-threaded, on synthetic data (`cargo bench -p logdrain --bench add`):

| operation | latency |
|---|---|
| steady-state `add` (warm tree) | ~370 ns |
| cold-start `add` (every line new) | ~1.8 µs |
| `match_only` (read-only classify) | ~290 ns |

The `highvolume` example ingests **5M lines in ~2.9s (~1.7M lines/sec)**, collapsing
to 8 templates. Streaming a file through the CLI holds the whole process in single-digit
MB regardless of line count, because the miner retains templates, not lines.

The concurrency model shards the tree by token count (each shard behind its own
lock) so independent shapes don't contend - but the numbers above are single-thread;
a multi-core benchmark isn't in yet.

## MSRV & license

Rust 1.80+. Licensed under either of Apache-2.0 or MIT, at your option.
