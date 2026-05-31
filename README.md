# logdrain

High-throughput, online log template mining in Rust (Drain3 algorithm).

> v0.1: core library only — `Miner`, `Cluster`, whitespace tokenizer,
> token-count-sharded prefix tree, similarity matching, per-leaf LRU, and
> sync snapshot/restore. Path-preserving tokenization, masks, the CLI, and the
> `draind` service land in later releases.

## Quick start

```rust
use logdrain::Miner;

let miner = Miner::builder().build().unwrap();
let r = miner.add("user 42 logged in");
println!("cluster {} ({:?})", r.cluster_id, r.update);
```

Run the example:

```sh
cargo run -p logdrain --example basic
```

## License

Licensed under either of Apache-2.0 or MIT at your option.
