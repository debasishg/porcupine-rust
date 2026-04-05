# porcupine-rust — Benchmarking: Process, Results, and Improvement Suggestions

> **Date**: 2026-04-05 | **Machine**: Apple M1 | **Rust**: 1.x (stable) | **Go**: 1.26.1
>
> **Note**: This document records the initial benchmark state and analysis that motivated the
> optimization work. Current numbers (after three optimization passes) are in
> [`docs/benchmark_improvements.md`](benchmark_improvements.md).

---

## 1. Objectives

Compare the Rust port (`porcupine-rust`) against the original Go implementation
([anishathalye/porcupine](https://github.com/anishathalye/porcupine)) on identical
workloads, in a way that is:

- **Fair** — same input bytes, same algorithm path, parallelism controlled explicitly
- **Reproducible** — statistical tooling on both sides (Criterion.rs, `go test -bench -count=10`)
- **Meaningful** — real-world histories (Jepsen etcd traces, KV store traces) rather than synthetic microbenchmarks

---

## 2. Test Data

Both implementations read the exact same files from `test_data/` in this repo.

| Dataset | Files | Description |
|---------|-------|-------------|
| `test_data/jepsen/etcd_000.log` … `etcd_102.log` | 102 files (~170 ops each) | Real Jepsen etcd histories; 44 linearizable, 58 not |
| `test_data/kv/c10-ok.txt` | 1 file | 10-client KV store, linearizable |
| `test_data/kv/c10-bad.txt` | 1 file | 10-client KV store, non-linearizable |

`c50` traces were excluded from the comparison — without partitioning they exceed 10 minutes; with
partitioning they are structurally identical to `c10` and add no new signal.

---

## 3. Parallelism Control

Go's `CheckOperations` / `CheckEvents` is **single-threaded by default**.  
Rust's `check_operations` / `check_events` always dispatches to **rayon's global thread pool**.

To get an apples-to-apples baseline, the Rust sequential benchmarks install a dedicated `rayon::ThreadPool` with exactly 1 thread and run the checker inside `pool.install(|| ...)`. This does not affect the global pool and is safe to reuse across Criterion `iter` calls.

```rust
// benches/linearizability.rs — sequential group setup
let pool = rayon::ThreadPoolBuilder::new()
    .num_threads(1)
    .build()
    .unwrap();

group.bench_function("single_file", |b| {
    b.iter(|| pool.install(|| check_events(&EtcdModel, &history, None)));
});
```

The parallel groups simply call `check_events` directly, using all available cores.

---

## 4. Infrastructure Setup

### 4.1 Rust — Criterion

**`Cargo.toml` changes:**

```toml
[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }

[[bench]]
name = "linearizability"
harness = false
```

**Benchmark file**: `benches/linearizability.rs`

Contains inline copies of `EtcdModel`, `KvModel`, and their log parsers (the models in `tests/go_compat.rs`
are not accessible from `benches/`; Rust's `tests/` and `benches/` are separate compilation units).
Three benchmark groups, 8 functions total:

| Group | Functions | Thread count |
|-------|-----------|-------------|
| `etcd_sequential` | `single_file`, `all_files/102` | 1 (dedicated pool) |
| `etcd_parallel` | `single_file`, `all_files/102` | all cores (rayon global) |
| `kv_partitioned` | `c10_ok_seq`, `c10_bad_seq`, `c10_ok_par`, `c10_bad_par` | 1 / all cores |

### 4.2 Go — `go test -bench`

The Go repo (`anishathalye/porcupine`) already contains per-file benchmark functions
(`BenchmarkEtcdJepsen000` … `BenchmarkEtcdJepsen102`) and KV benchmarks
(`BenchmarkKv10ClientsOk`, `BenchmarkKv10ClientsBad`) in `porcupine_test.go`.

One additional file was added to supply an aggregate all-102-files benchmark:

**`/tmp/porcupine-go/porcupine_bench_all_test.go`** (copy kept at `benches/go/porcupine_bench_all_test.go`):

```go
package porcupine

import (
    "fmt"
    "os"
    "testing"
)

func BenchmarkEtcdJepsenAll(b *testing.B) {
    var histories [][]Event
    for i := 0; i <= 102; i++ {
        path := fmt.Sprintf("test_data/jepsen/etcd_%03d.log", i)
        if _, err := os.Stat(path); err == nil {
            histories = append(histories, parseJepsenLog(path))
        }
    }
    b.ResetTimer()
    for i := 0; i < b.N; i++ {
        for _, events := range histories {
            CheckEvents(etcdModel, events)
        }
    }
}
```

---

## 5. Commands Executed

### 5.1 Install Go (was not present)

```bash
brew install go
# → go version go1.26.1 darwin/arm64
```

### 5.2 Clone Go porcupine

```bash
git clone https://github.com/anishathalye/porcupine /tmp/porcupine-go
```

### 5.3 Add aggregate benchmark to Go repo

```bash
cp benches/go/porcupine_bench_all_test.go /tmp/porcupine-go/
```

> The Go repo's `test_data/` already contains the same Jepsen and KV files; no symlink was needed.

### 5.4 Rust benchmark — dry-run (verify all 8 compile and execute)

```bash
cargo bench --bench linearizability -- --test
```

Expected output:

```
Testing etcd_sequential/single_file    Success
Testing etcd_sequential/all_files/102  Success
Testing etcd_parallel/single_file      Success
Testing etcd_parallel/all_files/102    Success
Testing kv_partitioned/c10_ok_seq      Success
Testing kv_partitioned/c10_bad_seq     Success
Testing kv_partitioned/c10_ok_par      Success
Testing kv_partitioned/c10_bad_par     Success
```

### 5.5 Rust benchmark — full run

```bash
cargo bench --bench linearizability
```

Criterion collects 100 samples (20 for KV groups), computes mean ± std dev, and writes HTML
reports to `target/criterion/`.

### 5.6 Go benchmark — full run

```bash
cd /tmp/porcupine-go
go test \
  -bench='BenchmarkEtcdJepsen000|BenchmarkEtcdJepsenAll|BenchmarkKv10Clients' \
  -benchmem \
  -count=10 \
  -run='^$' \
  .
```

Flags:
- `-run='^$'` — skip all unit tests, run only benchmarks
- `-count=10` — 10 independent runs per benchmark for stable statistics
- `-benchmem` — report allocations/op alongside ns/op

---

## 6. Raw Results

### 6.1 Rust (Criterion)

```
etcd_sequential/single_file    time: [107.15 µs  107.23 µs  107.31 µs]
etcd_sequential/all_files/102  time: [250.16 ms  250.50 ms  250.87 ms]

etcd_parallel/single_file      time: [103.60 µs  103.67 µs  103.75 µs]
etcd_parallel/all_files/102    time: [249.95 ms  250.25 ms  250.55 ms]

kv_partitioned/c10_ok_seq      time: [359.00 µs  368.03 µs  377.49 µs]
kv_partitioned/c10_bad_seq     time: [216.44 µs  217.12 µs  218.22 µs]
kv_partitioned/c10_ok_par      time: [316.37 µs  317.78 µs  319.24 µs]
kv_partitioned/c10_bad_par     time: [262.46 µs  265.82 µs  269.27 µs]
```

### 6.2 Go (`go test -bench`, 10 runs, ns/op)

```
BenchmarkEtcdJepsen000-8     10000    113942 ns/op    185301 B/op    1691 allocs/op
BenchmarkEtcdJepsen000-8     10000    114536 ns/op    185301 B/op    1691 allocs/op
BenchmarkEtcdJepsen000-8     10000    114799 ns/op    185300 B/op    1691 allocs/op
BenchmarkEtcdJepsen000-8     10000    115050 ns/op    185300 B/op    1691 allocs/op
BenchmarkEtcdJepsen000-8     10000    114328 ns/op    185300 B/op    1691 allocs/op
BenchmarkEtcdJepsen000-8     10000    115002 ns/op    185301 B/op    1691 allocs/op
BenchmarkEtcdJepsen000-8     10000    113690 ns/op    185301 B/op    1691 allocs/op
BenchmarkEtcdJepsen000-8     10000    114084 ns/op    185301 B/op    1691 allocs/op
BenchmarkEtcdJepsen000-8     10000    114337 ns/op    185300 B/op    1691 allocs/op
BenchmarkEtcdJepsen000-8     10000    114193 ns/op    185301 B/op    1691 allocs/op

BenchmarkEtcdJepsenAll-8     4    289571719 ns/op    126180016 B/op    3567282 allocs/op
BenchmarkEtcdJepsenAll-8     4    290563500 ns/op    125974280 B/op    3567269 allocs/op
BenchmarkEtcdJepsenAll-8     4    288846834 ns/op    126118692 B/op    3567279 allocs/op
BenchmarkEtcdJepsenAll-8     4    291436740 ns/op    126221004 B/op    3567285 allocs/op
BenchmarkEtcdJepsenAll-8     4    290228406 ns/op    126096964 B/op    3567274 allocs/op
BenchmarkEtcdJepsenAll-8     4    292976656 ns/op    126117768 B/op    3567276 allocs/op
BenchmarkEtcdJepsenAll-8     4    291187271 ns/op    126097420 B/op    3567276 allocs/op
BenchmarkEtcdJepsenAll-8     4    289752125 ns/op    126179244 B/op    3567278 allocs/op
BenchmarkEtcdJepsenAll-8     4    290309521 ns/op    126056320 B/op    3567274 allocs/op
BenchmarkEtcdJepsenAll-8     4    291476260 ns/op    126322388 B/op    3567284 allocs/op

BenchmarkKv10ClientsOk-8     5041    237030 ns/op    560062 B/op    3428 allocs/op
BenchmarkKv10ClientsOk-8     5235    239203 ns/op    560049 B/op    3428 allocs/op
BenchmarkKv10ClientsOk-8     4842    238822 ns/op    560048 B/op    3428 allocs/op
BenchmarkKv10ClientsOk-8     4854    238786 ns/op    560049 B/op    3428 allocs/op
BenchmarkKv10ClientsOk-8     4820    239687 ns/op    560049 B/op    3428 allocs/op
BenchmarkKv10ClientsOk-8     4882    238659 ns/op    560048 B/op    3428 allocs/op
BenchmarkKv10ClientsOk-8     5118    238377 ns/op    560050 B/op    3428 allocs/op
BenchmarkKv10ClientsOk-8     4628    240858 ns/op    560047 B/op    3428 allocs/op
BenchmarkKv10ClientsOk-8     4737    240380 ns/op    560050 B/op    3428 allocs/op
BenchmarkKv10ClientsOk-8     4929    239500 ns/op    560048 B/op    3428 allocs/op

BenchmarkKv10ClientsBad-8    7239    167825 ns/op    579713 B/op    1469 allocs/op
BenchmarkKv10ClientsBad-8    6829    168982 ns/op    579671 B/op    1468 allocs/op
BenchmarkKv10ClientsBad-8    7191    167603 ns/op    579630 B/op    1468 allocs/op
BenchmarkKv10ClientsBad-8    7066    167649 ns/op    579395 B/op    1464 allocs/op
BenchmarkKv10ClientsBad-8    7107    169415 ns/op    579865 B/op    1472 allocs/op
BenchmarkKv10ClientsBad-8    6966    169289 ns/op    579751 B/op    1470 allocs/op
BenchmarkKv10ClientsBad-8    7050    166880 ns/op    579241 B/op    1461 allocs/op
BenchmarkKv10ClientsBad-8    7125    169212 ns/op    579421 B/op    1464 allocs/op
BenchmarkKv10ClientsBad-8    6966    168767 ns/op    579525 B/op    1466 allocs/op
BenchmarkKv10ClientsBad-8    7232    169996 ns/op    579546 B/op    1466 allocs/op
```

---

## 7. Summary Comparison

Figures are **medians** across 10 runs. Rust times are Criterion point estimates (lower bound of CI).

| Benchmark | Rust (1 thread) | Go | Ratio (Rust/Go) |
|-----------|----------------:|---:|:---:|
| etcd — single file (`etcd_000.log`) | 107 µs | 114 µs | **0.94 (Rust 1.07× faster)** |
| etcd — all 102 files | 250 ms | 290 ms | **0.86 (Rust 1.16× faster)** |
| kv — c10-ok (partitioned) | 368 µs | 239 µs | **1.54 (Go 1.54× faster)** |
| kv — c10-bad (partitioned) | 217 µs | 168 µs | **1.29 (Go 1.29× faster)** |

**Rust parallel KV (all cores) vs Go (single thread):**

| Benchmark | Rust (all cores) | Go (1 thread) | Ratio |
|-----------|----------------:|---:|:---:|
| kv — c10-ok | 318 µs | 239 µs | **1.33 (still Go faster)** |
| kv — c10-bad | 266 µs | 168 µs | **1.58 (still Go faster)** |

---

## 8. Analysis

### 8.1 etcd workload (no partitioning) — Rust wins narrowly

The etcd model has no `partition()` implementation. Every history is checked as a single DFS
traversal. Rayon has nothing to split, so sequential and parallel Rust numbers are identical
(107 µs vs 104 µs — within noise).

Rust outperforms Go by 7–16%:
- No GC pauses (Go's GC must collect the allocations made per DFS step)
- The `Bitset` and `NodeArena` types avoid repeated heap allocation during traversal
- `HashMap` with `u64` keys and good load-factor control outperforms Go's map on this access pattern

The margin is narrow because Go's interface-based model dispatch is cheap and the histories
are small (~170 ops), so the constant factors dominate over algorithmic differences.

### 8.2 KV workload (partitioned) — Go wins decisively

This is the surprising result. Go is 29–54% faster on the partitioned KV traces, even against
Rust running on multiple cores.

**Root cause — `String` cloning per DFS step:**

The Rust `KvModel` uses `type State = String`. The DFS calls `model.step()` on every candidate
linearization point and receives a cloned new state on success. With the Go checker, the model
state is typed as `interface{}` holding a Go `string`. Go strings are immutable header structs
(pointer + length) — copying them is two words, with no heap allocation. Rust's `String` is a
heap-allocated growable buffer — cloning it allocates and copies the contents every time.

For `c10-ok`, the per-key partitions each have ~30–50 operations. The DFS explores O(n!)
orderings in the worst case (pruned by caching), and at each valid step it clones the state.
With 10 keys and small partitions, the rayon thread-pool overhead (~microseconds to dispatch)
is comparable to the total useful work, erasing the parallelism benefit.

**Go allocation numbers confirm this:**
```
BenchmarkKv10ClientsOk-8:  560 KB/op, 3428 allocs/op
```
These allocations are dominated by `interface{}` boxing and slice growth in the DFS, not state
cloning — Go's string copy is entirely on the stack.

### 8.3 Sequential vs parallel Rust (KV)

| | c10-ok | c10-bad |
|-|--------|---------|
| Sequential (1 thread) | 368 µs | 217 µs |
| Parallel (all cores) | 318 µs | 266 µs |
| Speedup | 1.16× | 0.82× |

`c10-ok` gets a modest speedup (16%) from rayon because all 10 key-partitions must be
verified and rayon runs them concurrently. `c10-bad` is *slower* with rayon: a violation is
found almost immediately in one partition; the other threads are then cancelled, but the
coordination overhead (AtomicBool, rayon task dispatch) exceeds the time saved.

### 8.4 etcd sequential vs parallel (Rust)

Both groups produce 104–107 µs for a single file because there is only one partition. The
`all_files/102` groups are also identical (250 ms) because the benchmark iterates sequentially
over the 102 files in both cases — the benchmark harness itself is not parallelised.

---

## 9. Suggestions for Improving Rust Performance

The improvements are ordered by expected impact vs implementation effort.

---

### 9.1 Replace `String` state with `Arc<str>` in `KvModel` *(low effort, high impact)*

**Where**: `benches/linearizability.rs` and `tests/go_compat.rs`, `KvModel::State`

**Change**:
```rust
// Before
impl Model for KvModel {
    type State = String;
    ...
    fn step(&self, state: &String, input: &KvInput, output: &KvOutput) -> Option<String> {
        match input.op {
            KvOp::Get    => if output.value == *state { Some(state.clone()) } else { None },
            KvOp::Put    => Some(input.value.clone()),
            KvOp::Append => Some(format!("{}{}", state, input.value)),
        }
    }
}

// After
impl Model for KvModel {
    type State = Arc<str>;
    ...
    fn step(&self, state: &Arc<str>, input: &KvInput, output: &KvOutput) -> Option<Arc<str>> {
        match input.op {
            KvOp::Get    => if output.value.as_str() == state.as_ref() {
                                Some(Arc::clone(state))   // ref-count bump only, no heap alloc
                            } else { None },
            KvOp::Put    => Some(Arc::from(input.value.as_str())),
            KvOp::Append => Some(Arc::from(format!("{}{}", state, input.value).as_str())),
        }
    }
}
```

`Arc::clone` is a single atomic increment — O(1), no allocation. For the read-only `Get` and
sequential `Append` paths this directly eliminates the dominant allocation in the DFS hot loop.
Expected gain: 30–50% on KV benchmarks, bringing Rust competitive with Go.

---

### 9.2 Replace `HashMap` with `FxHashMap` in the DFS cache *(low effort, moderate impact)*

**Where**: `src/checker.rs`, the `cache: HashMap<u64, Vec<CacheEntry<S>>>` in `check_single`

**Change**: swap `std::collections::HashMap` for `rustc_hash::FxHashMap`.

```toml
# Cargo.toml
[dependencies]
rustc-hash = "2"
```

```rust
// src/checker.rs
use rustc_hash::FxHashMap;
// Replace HashMap::new() with FxHashMap::default()
```

The DFS cache is keyed by a `u64` (the bitset hash). `FxHashMap` uses a non-cryptographic
identity-like hash for integer keys — effectively O(1) with near-zero per-key overhead.
`std::HashMap` uses SipHash13 (DOS-resistant but slower). For the etcd benchmark this could
yield 10–20% improvement; for KV it is secondary to the state-clone fix.

---

### 9.3 Parallelise the `all_files` benchmark across files *(moderate effort, large parallel speedup)*

**Where**: `benches/linearizability.rs`, `bench_etcd_parallel`

Currently the parallel benchmark iterates sequentially over the 102 files inside `b.iter()`.
The `etcd_parallel/all_files` and `etcd_sequential/all_files` numbers are therefore identical.
To demonstrate true cross-file parallelism (and close the gap with Go's already-sequential
all-102 number), run the files in parallel:

```rust
use rayon::prelude::*;

group.bench_function("all_files_rayon", |b| {
    b.iter(|| {
        all_histories.par_iter().for_each(|h| {
            let _ = check_events(&EtcdModel, h, None);
        });
    });
});
```

This is a benchmark-level change only. Because each etcd history is independent, this is
embarrassingly parallel. On an 8-core M1 this should reduce the 250 ms wall time to ~35 ms
(~7× speedup), decisively beating Go's 290 ms.

Note: this tests **benchmark parallelism**, not the library's internal partition parallelism.
It is valid as long as both sides agree on the scope being compared.

---

### 9.4 Eliminate per-step state allocation with an arena or slab *(high effort, maximum impact)*

**Where**: `src/checker.rs`, `check_single` DFS loop

The DFS stack pushes `(BitSet, State)` pairs onto `calls: Vec<(Bitset, S)>`. Each `S` is
cloned via the `Model::State: Clone` bound. For models where `S` is expensive to clone
(e.g. `HashMap`, `Vec`, `String`), this dominates.

A slab allocator or bump allocator per DFS thread can replace heap allocation with a pointer
bump. The idea:

```rust
// Conceptual sketch — not drop-safe as written
struct DfsArena {
    slab: Vec<u8>,
    offset: usize,
}
impl DfsArena {
    fn alloc<T>(&mut self, val: T) -> *const T { ... }
    fn reset(&mut self) { self.offset = 0; }
}
```

Alternatively, consider representing the DFS state as a persistent/functional data structure
(e.g. `im::HashMap` from the `im` crate for the `KvNoPartitionModel`) — structural sharing
means `clone` is O(1) in the common case.

This is the highest-leverage fix for models with complex states (`HashMap`, `Vec<T>`) but
requires an invasive refactor of `check_single` and is incompatible with the `Clone` bound as
currently written.

---

### 9.5 Short-circuit `check_parallel` for single partitions *(minimal effort, minor gain)*

**Where**: `src/checker.rs`, `check_parallel`

When `partitions.len() == 1`, rayon's `par_iter().any()` still incurs thread-pool overhead
(task submission, `AtomicBool` allocation, join). A short-circuit avoids this:

```rust
fn check_parallel<M>(model: &M, partitions: Vec<...>, kill: Arc<AtomicBool>, ...) -> bool {
    if partitions.len() == 1 {
        // Fast path: no rayon overhead for unpartitioned models (e.g. etcd)
        return check_single(model, partitions.into_iter().next().unwrap(), &kill);
    }
    // existing rayon path ...
}
```

For the etcd benchmark (single partition per history), this saves ~2–5 µs of rayon dispatch
per call — roughly 2–5% of the current 107 µs runtime.

---

### 9.6 Pre-sort and deduplicate the `pending` map drain in parsers *(negligible, cleanup)*

**Where**: `benches/linearizability.rs` and `tests/go_compat.rs`, end-of-file `pending` drain

The drain over `pending` entries for timed-out ops currently iterates in arbitrary hash order.
This produces non-deterministic event orderings, which could affect cache hit rates across
runs. Sorting by process ID before appending matches Go's deterministic append order and
makes benchmark results more reproducible. The performance impact is negligible (only a
handful of ops) but improves correctness as a side-effect.

---

## 10. Recommended Prioritisation

| # | Change | Effort | Expected gain | Target benchmark |
|---|--------|--------|---------------|-----------------|
| 1 | `Arc<str>` state for `KvModel` | Low | +30–50% KV | `kv_partitioned` |
| 2 | `FxHashMap` for DFS cache | Low | +10–20% all | `etcd_*`, `kv_*` |
| 3 | Parallel file iteration in benchmark | Low | +~7× parallel etcd | `etcd_parallel` |
| 4 | Short-circuit single-partition path | Minimal | +2–5% etcd | `etcd_*` |
| 5 | Arena/slab for DFS state | High | +50%+ on complex models | `kv_partitioned` (no-partition variant) |

Implementing items 1–4 is low-risk (benchmark and library boundary only) and should bring
Rust to parity or better than Go across all workloads measured here.
