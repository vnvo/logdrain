# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.2] - 2026-06-28

### Changed

- Added crates.io `keywords` and `categories` to both crates for discoverability
  (lib.rs / crates.io search). No code changes.

## [0.3.1] - 2026-06-28

### Changed

- Docs: clarified that persistence is a **library** API (`save_state` / `load_state`) -
  caller-driven, full-snapshot, not exposed in the CLI - and named the available backends
  (in-memory, file, Redis, Kafka).

### Tests

- Added functional tests for the Redis and Kafka backends using **testcontainers** (real
  brokers via Docker), run in CI. Removed the redundant env-var-based ignored tests.

## [0.3.0] - 2026-06-28

### Added

- **Redis persistence backend** (`RedisPersistence`, cargo feature `redis`): stores the
  latest snapshot under a single key. Pure-Rust client, no system dependencies.
- **Kafka persistence backend** (`KafkaPersistence`, cargo feature `kafka`): writes the
  snapshot to the tail of a topic and reads it back from partition 0 — use a compacted,
  single-partition topic for snapshot semantics. Built on `rdkafka`; enabling the feature
  requires a C toolchain and links librdkafka.

### Changed

- Wording: logdrain is described as an independent Rust implementation of the **Drain**
  algorithm (He et al., 2017) with the practical extensions Drain3 popularized, rather
  than "the Drain3 algorithm".

### Notes

- Both backends are **off by default** — the core crate stays dependency-light.

## [0.2.0] - 2026-06-28

### Added

- **Event-time tracking.** `Miner::add_at` / `add_with_member_at` fold a caller-supplied
  event timestamp (unix-ms) into each cluster. New accessors `Cluster::event_first_seen`,
  `event_last_seen`, and `event_lines_per_minute` report true event-time first/last-seen
  and per-template rates, independent of processing wall-clock — so replaying a historical
  file reports when events actually happened, not how fast it was fed.
- **CLI `--time-key` / `--time-format`.** Parse a per-record timestamp from a JSON field
  (dot-path). Without a format, integer Unix epochs (seconds / millis / micros / nanos,
  auto-scaled by magnitude) and RFC 3339 strings are auto-detected; `--time-format`
  accepts any [chrono strftime](https://docs.rs/chrono/latest/chrono/format/strftime/index.html)
  pattern. Missing or unparseable values are still clustered, with a count reported on
  stderr. Output gains `eventFirstSeen` / `eventLastSeen` / `eventRatePerMinute`
  (JSON / JSONL) and matching CSV columns.
- **CLI multi-line record assembly.** `--multiline-start <REGEX>` begins a new record at
  each matching line (delimiter-less logs such as raw stack traces); `--record-separator
  <SEP>` splits the stream on an explicit separator (blank line, NUL byte, sentinel;
  common escapes interpreted). The two are mutually exclusive.
- **CLI `--format jsonl`** (alias `ndjson`): one compact JSON object per line, for
  streaming into jq, Vector, Logstash, Kafka, or Elasticsearch.

### Changed

- Snapshot format bumped to **v3** (adds the event-time fields). v2 snapshots are read and
  upgraded automatically (event time defaults to unset); v1 is rejected.
- The first-line-mode suffix is now documented as **masked** — it passes through the same
  mask pass as the rest of the record — correcting the earlier "verbatim" wording.
- Clarified documentation throughout: the CLI input model (one record per line, and how to
  assemble multi-line records), stack-trace masking, what first-line mode does, and that
  `createdAt` / `updatedAt` / `linesPerMinute` are processing observation time, not parsed
  log time.

### Dependencies

- `logdrain-cli` now depends on `chrono` (for `--time-format`). The `logdrain` library
  remains dependency-light and parses no timestamps itself — `add_at` takes a `u64`.

## [0.1.1] - 2026-06-27

### Added

- README bundled inside the published crates; MSRV (1.85) declared in crate metadata.

### Fixed

- Release publishing is idempotent (skips versions already on crates.io).
- Clippy: use `is_none_or` instead of `map_or(true, ..)`.

## [0.1.0] - 2026-06-27

- Initial release: an online Drain log-template miner as a library (`logdrain`) and a CLI
  (`logdrain-cli`). Path-preserving tokenization, configurable masks, stack-trace
  clustering, exact per-template counts, snapshot/restore persistence, and a thread-safe
  sharded hot path.

[0.3.2]: https://github.com/vnvo/logdrain/releases/tag/v0.3.2
[0.3.1]: https://github.com/vnvo/logdrain/releases/tag/v0.3.1
[0.3.0]: https://github.com/vnvo/logdrain/releases/tag/v0.3.0
[0.2.0]: https://github.com/vnvo/logdrain/releases/tag/v0.2.0
[0.1.1]: https://github.com/vnvo/logdrain/releases/tag/v0.1.1
[0.1.0]: https://github.com/vnvo/logdrain/releases/tag/v0.1.0
