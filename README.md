# porcupine-rust

A Rust port of [porcupine](https://github.com/anishathalye/porcupine), a fast linearizability checker for testing the correctness of concurrent and distributed systems.

## What is Linearizability?

Linearizability is a correctness condition for concurrent systems. A history of concurrent operations is linearizable if the operations can be reordered — while respecting their real-time overlap — into a sequential execution that satisfies the system's sequential specification.

## Features

- Check linearizability of concurrent operation histories against a sequential model
- Support for both timestamped `Operation` and raw `Event` (call/return) history formats
- Optional timeout-based checking with a tri-state `CheckResult`
- P-compositional checking for partitionable models (e.g., key-value stores partitioned by key)
- Efficient DFS with backtracking, bitset-based state tracking, and caching
- `NondeterministicModel` trait + `PowerSetModel` adapter for models with branching step semantics (e.g. lossy writes, replica reads, internal non-observable choices)

## Usage

```rust
use porcupine::{CheckResult, Model, Operation};
use std::time::Duration;

// Define a sequential model (e.g., a register)
// ...

// Unbounded check
let result = porcupine::checker::check_operations(&model, &history, None);
assert_eq!(result, CheckResult::Ok);

// Bounded check — returns Unknown if the DFS does not finish in time
let result = porcupine::checker::check_operations(&model, &history, Some(Duration::from_secs(5)));
assert!(matches!(result, CheckResult::Ok | CheckResult::Unknown));
```

## Benchmarks

Benchmarks are run with [Criterion.rs](https://github.com/bheisler/criterion.rs) on the Rust side and `go test -bench` on the Go side, using identical byte-for-byte input files:

- **102 Jepsen etcd log files** — each a short single-partition history (~10–30 operations); representative of real Jepsen runs against etcd.
- **6 KV store traces** — multi-key histories that exercise P-compositional checking; `c10-ok` is a correct 10-client trace, `c10-bad` contains a linearizability violation.

**Parallelism control**: the Rust sequential benchmarks use a dedicated `rayon::ThreadPool` with `num_threads(1)`, so sequential Rust and single-threaded Go are genuinely apples-to-apples. The parallel benchmarks use the default rayon thread pool (one thread per logical core).

### Results (Apple M1, 3-run average)

| Benchmark | Rust | Go | Speedup |
|-----------|------|----|---------|
| etcd — single file (sequential) | 40.8 µs | 114 µs | **2.8×** |
| etcd — 102 files (sequential) | 145.1 ms | 290 ms | **2.0×** |
| etcd — single file (parallel) | 31.9 µs | 114 µs | **3.6×** |
| etcd — 102 files (parallel) | 77.8 ms | 290 ms | **3.7×** |
| kv `c10-ok` (sequential) | 180.4 µs | 239 µs | **1.32×** |
| kv `c10-bad` (sequential) | 84.7 µs | 168 µs | **1.98×** |
| kv `c10-ok` (parallel) | 175.5 µs | 239 µs | **1.36×** |
| kv `c10-bad` (parallel) | 79.8 µs | 168 µs | **2.10×** |

Rust leads Go on every benchmark. The key contributors are: compact `Node` struct with `u32` indices and sentinel-based linked-list (3× smaller index overhead per node, better cache-line utilization); deferred bitset clone with incremental hash computation (clone only on cache miss, `hash_with_bit()` avoids O(chunks) scan); `FxHashMap` for the DFS cache (replacing SipHash); `SmallVec<[u64; 4]>` for the bitset (zero heap allocation for ≤ 256 operations); `SmallVec<[CacheEntry; 2]>` for the DFS cache collision list (eliminates heap allocation for the common 0–1 collision case); `Arc<str>` for KV model state (atomic refcount bump instead of `String` clone on every DFS step); a single-partition fast path that skips rayon dispatch; a sequential fallback for small inputs (< 2000 total entries); and `#[inline]` hints on the hot-path `lift`/`unlift`/`cache_contains` functions called thousands of times per history check.

### Results (Apple M5 Pro, 3-run average)

| Benchmark | Rust | Go | Speedup |
|-----------|------|----|---------|
| etcd — single file (sequential) | 24.6 µs | 90 µs | **3.7×** |
| etcd — 102 files (sequential) | 87.6 ms | 267 ms | **3.0×** |
| etcd — single file (parallel) | 17.5 µs | 90 µs | **5.2×** |
| etcd — 102 files (parallel) | 45.3 ms | 267 ms | **5.9×** |
| kv `c10-ok` (sequential) | 111.0 µs | 188 µs | **1.7×** |
| kv `c10-bad` (sequential) | 50.9 µs | 93 µs | **1.8×** |
| kv `c10-ok` (parallel) | 100.5 µs | 188 µs | **1.9×** |
| kv `c10-bad` (parallel) | 43.5 µs | 93 µs | **2.1×** |

The M5 Pro gives both languages a ~1.3–1.8× raw speedup over M1. Rust benefits more from M5's wider execution pipeline — the etcd parallel all-files benchmark improved from 3.5× → 5.9× advantage over Go. The KV partitioned benchmarks also widened their Rust-over-Go margin slightly.

## Status

Complete — all core features of the [original Go implementation](https://github.com/anishathalye/porcupine) are ported. See [SKILLS.md](SKILLS.md) for the self-verified pipeline (Quint formal model, property tests, model-based tests).

## License

MIT — see [LICENSE](LICENSE).
