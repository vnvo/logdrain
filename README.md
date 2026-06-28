<div align="center">

# logdrain

**High-throughput, online log-template mining in Rust.**

An independent Rust implementation of the **Drain** log-mining algorithm (He et al., 2017) - with the practical extensions Drain3 popularized (masks, persistence, numeric parametrization) plus path-preserving tokenization and stack-trace clustering - as an embeddable library and a CLI.

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

#### Event-time aware
Feed a parsed timestamp per line (`add_at`, or the CLI's `--time-key`) for true event-time first/last-seen and rates, independent of processing wall-clock.

#### Exhaustive & persistent
Exact per-template counts (no sampling, no top-N) with `snapshot`/`restore` to a pluggable backend - in-memory, file, or Redis/Kafka behind features.

#### Thread-safe & fast
`add(&self)` from many threads; a sharded, mostly-lock-free hot path (~1-2M lines/sec per core).

## 💡 Use cases

#### Noise reduction & triage
Collapse millions of lines into a handful of templates, ranked by volume, to see what's *actually* happening.

#### New-pattern alerting
Alert on the `Created` signal, the first time a never-seen log shape appears (new errors, attacks, regressions).

#### Incident & exception clustering
Rank distinct stack-trace failures by frequency, so you triage the top one instead of scrolling.

#### Pipeline cost reduction
Store `(template, count)` instead of raw lines, or sample by template - a Vector/Cribl-style stage.

#### Structured extraction
Turn unstructured logs into `(template_id, params)` for dashboards, ML features, or alerting.

#### Context for AI agents
Compress millions of lines into a few hundred templates + counts so an LLM or incident-triage
agent can reason over the whole picture - feed the catalog and the `Created` signal, not raw lines.

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
logdrain --format jsonl app.log | jq -c '{template, size}'   # stream one object/line into jq, Vector, Kafka, ...
```

> **`createdAt` is observation time, not log time.** The `createdAt` / `updatedAt`
> fields (and the derived `linesPerMinute` / `rate_per_min`) are the machine's
> wall-clock when logdrain *first/last saw* each template - logdrain does **not** parse
> timestamps from your records by default. Streaming a live feed they approximate event
> time; replaying a historical file they reflect *when you ran the command* and how fast
> you fed it, not the original event times or rate.
>
> For **true event time**, pass `--time-key <field>` (with optional `--time-format`).
> logdrain then parses that timestamp out of each record and reports
> `eventFirstSeen` / `eventLastSeen` / `eventRatePerMinute` - which reflect when events
> actually happened, even when replaying historical logs:
>
> ```sh
> logdrain --key msg --time-key ts app.ndjson --format jsonl   # epoch or RFC 3339, auto-detected
> logdrain --key m --time-key t --time-format '%Y-%m-%d %H:%M:%S' app.ndjson
> ```

<details>
<summary><b>Timestamp formats accepted by <code>--time-key</code></b></summary>

**Auto-detected** (no `--time-format` needed) - covers most structured JSON logs:

| Value | Example | |
|---|---|---|
| Unix epoch, seconds | `1700000000` | ✅ |
| Unix epoch, milliseconds | `1700000000000` | ✅ |
| Unix epoch, microseconds | `1700000000000000` | ✅ |
| Unix epoch, nanoseconds | `1700000000000000000` | ✅ (unit auto-scaled by magnitude) |
| RFC 3339 / ISO 8601, `Z` | `2024-01-01T12:30:00Z` | ✅ |
| RFC 3339, fraction + offset | `2024-01-01T12:30:00.5+02:00` | ✅ |
| Space-separated, no offset | `2024-01-01 12:30:00` | ❌ use `--time-format` |
| Date only | `2024-01-01` | ❌ (no time-of-day) |

**With `--time-format <FMT>`** - any [chrono strftime](https://docs.rs/chrono/latest/chrono/format/strftime/index.html) pattern:

| Format | Matches | |
|---|---|---|
| `%Y-%m-%d %H:%M:%S` | `2024-01-01 12:30:00` | ✅ (no offset → assumed **UTC**) |
| `%d/%b/%Y:%H:%M:%S %z` | `02/Jan/2024:12:30:00 +0000` | ✅ (Apache / nginx access logs) |
| `%s` | `1700000000` | ✅ (epoch via format) |
| `%Y-%m-%d` | `2024-01-01` | ❌ (a value must have **both date and time**) |

**Rules:**
- A value must contain a full **date *and* time**; date-only values do not parse.
- A pattern with an offset (`%z` / `%:z`) is honored; without one, the time is read as **UTC**.
- The field value must be the **whole** timestamp - logdrain does not pull a timestamp out of the middle of a larger string. For JSON that's natural (`--time-key` points at a clean value).
- Records whose timestamp is missing or unparseable are still clustered, just without event time; a count is printed to stderr.

</details>

<details>
<summary><b>All CLI options</b></summary>

| Option | Description |
|---|---|
| `--format text\|json\|jsonl\|csv` | output format (default `text`); `jsonl` (alias `ndjson`) is one JSON object per line for piping into other tools |
| `--key <FIELD>` | parse each line as JSON and extract this dot-path field |
| `--masks <names>` | comma list of built-ins: `uuid,hex32,email,ipv4,jwt` |
| `--path-delimiters <CHARS>` | characters preserved as token boundaries, e.g. `/` |
| `--first-line-only` | cluster on the first line; keep the rest as a suffix |
| `--record-separator <SEP>` | split records on a literal separator (e.g. `\n\n`, `\0`) instead of newlines |
| `--multiline-start <REGEX>` | start a new record at each line matching REGEX; other lines continue it |
| `--time-key <FIELD>` | JSON dot-path to a per-record event timestamp; enables true event-time rates |
| `--time-format <FMT>` | chrono format for the `--time-key` value (else epoch / RFC 3339 auto-detected) |
| `--sort size\|template\|id\|rate` | ordering (default `size`) |
| `--min-size <N>` | hide clusters smaller than N |
| `--sim-th <FLOAT>` | similarity threshold (default `0.4`) |
| `--depth <N>` | prefix-tree depth (default `4`) |

</details>

### Input model

By default the CLI treats **one line as one record**. That already covers the common
cases:

- **Plaintext** logs (one event per line).
- **NDJSON** - one JSON object per line; use `--key <field>` to pull the message out.
- **A stack trace escaped inside a JSON field** - since the whole event is still one
  physical line, `--key stack --first-line-only` splits on the embedded `\n` and clusters
  on the first line.

When a single record spans **multiple physical lines** (raw console stack traces,
pretty-printed JSON), pick one of two options - **which one depends on whether the stream
already marks record boundaries:**

**1. `--record-separator <SEP>` - when records are explicitly framed.**
Use it when a known delimiter sits between records (or you control the producer and can
emit one): a blank line, a NUL byte, a sentinel string. Exact and unambiguous. Escapes
`\n \r \t \0 \\` are interpreted.

```sh
logdrain --record-separator '\n\n' app.log     # records separated by a blank line
producer | logdrain --record-separator '\0'     # NUL-framed stream
```

**2. `--multiline-start <REGEX>` - when there is no delimiter.**
Use it for ordinary logs you don't control, where a record *begins* with a recognizable
line (a timestamp or level) and continuation lines (stack frames) follow. A line matching
the regex starts a new record; every non-matching line is appended to it.

```sh
logdrain --multiline-start '^\d{4}-\d{2}-\d{2}' app.log   # records start at a timestamp
logdrain --multiline-start '^(ERROR|WARN|INFO)' --first-line-only app.log
```

> **Rule of thumb:** if you can point at a character that separates records, use
> `--record-separator`. If the boundary is only implied by what the *first line* looks
> like (the usual case for tracebacks), use `--multiline-start`. The two are mutually
> exclusive. Embedding the library? Skip both - *you* decide record boundaries and pass
> each whole record (newlines and all) to `add()`.

## 🧵 Stack traces

**First-line mode** (`--first-line-only`, or `.first_line_only(true)`) changes *how much
of a record is used to cluster*. Off (default), the whole record is tokenized; on, the
record is split at the first newline and **only the first line** drives clustering and the
template - the rest is kept as a per-cluster suffix (from the first occurrence). It only
matters for records that span multiple lines.

That's exactly what stack traces need: the frames differ every time, but the first line
(exception type + message) is stable, so all occurrences collapse into one template (with
masks applied, so per-request ids / IPs become placeholders) while one full trace is
preserved as the suffix. The example below uses the **library** (each trace passed to
`add()` as one record); from the CLI, assemble multi-line traces with `--multiline-start`
or `--record-separator` (see [Input model](#input-model)):

```console
$ cargo run -p logdrain --example stacktrace
6 traces  ->  3 distinct failures

#1  x3  ERROR NullPointerException req <uuid> at com/acme/svc/OrderHandler.process
        at OrderHandler.process(OrderHandler.java:142)
        at Dispatcher.run(Dispatcher.java:88)
        at java.base/Thread.run(Thread.java:829)

#2  x2  ERROR SQLTimeoutException from <ipv4> at com/acme/db/ConnectionPool.acquire
        at ConnectionPool.acquire(ConnectionPool.java:64)
        at OrderHandler.load(OrderHandler.java:71)

#3  x1  WARN RetryableException at com/acme/net/HttpClient.call
        at HttpClient.call(HttpClient.java:33)
```

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

Persistence is a **library** API - `Miner::{save_state, load_state}`; the CLI does not
persist. Writes are caller-driven (no background flush) and each one is a *full* snapshot
of the current catalog, so you checkpoint on a timer, on shutdown, or every N lines.

```rust
use logdrain::{FilePersistence, Miner};

let store = FilePersistence::new("templates.bin");
miner.save_state(&store).unwrap();        // atomic write (tmp + fsync + rename)

let restored = Miner::builder().build().unwrap();
restored.load_state(&store).unwrap();     // rebuilds the tree from the snapshot
```

`Persistence` is a small sync trait. `MemoryPersistence` and `FilePersistence` ship in
core; `RedisPersistence` and `KafkaPersistence` are behind cargo features so the core
stays dependency-light, and `Miner::{snapshot, restore}` expose the raw bytes for any
other backend.

```toml
logdrain = { version = "0.3", features = ["redis"] }  # or "kafka", or both
```

```rust
// Redis: latest snapshot stored under one key.
let store = logdrain::RedisPersistence::new("redis://127.0.0.1/", "logdrain:snapshot")?;

// Kafka: snapshot written to the tail of a (compacted, single-partition) topic.
let store = logdrain::KafkaPersistence::new("localhost:9092", "logdrain-snapshots");

miner.save_state(&store)?;
miner.load_state(&store)?;
```

The `kafka` feature builds `rdkafka`, which links **librdkafka** - enabling it needs a C
toolchain (and the usual `libssl`/`libsasl2`/`libcurl` dev headers). The `redis` feature
is pure Rust with no system dependencies.

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

**Available:** `logdrain` (library) and `logdrain-cli`, published and tested, with
Redis and Kafka persistence backends behind cargo features.
**Roadmap:** an HTTP service (`draind`), an S3 persistence backend, and
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
