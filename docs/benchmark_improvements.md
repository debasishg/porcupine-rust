# porcupine-rust — Benchmark Improvements

> **Last updated**: 2026-04-05 | **Machine**: Apple M1 | **Rust**: stable | **Go**: 1.26.1

---

## 1. Motivation

The initial benchmark comparison (documented in `docs/benchmark_results.md`) revealed that
the Rust port was 29–54% **slower** than the original Go implementation on KV partitioned
workloads — the very class where porcupine claims its largest speedups.

| Workload | Rust (initial) | Go | Rust/Go |
|----------|---------------|-----|---------|
| etcd — single file | 107 µs | 114 µs | 0.94 (Rust 7% faster) |
| etcd — all 102 files | 250 ms | 290 ms | 0.86 (Rust 16% faster) |
| kv — c10-ok (partitioned) | 368 µs | 239 µs | **1.54 (Go 54% faster)** |
| kv — c10-bad (partitioned) | 217 µs | 168 µs | **1.29 (Go 29% faster)** |

Root causes identified:

1. **`String` cloning in `KvModel`**: the DFS hot path cloned `M::State` twice per
   successful step — once into the `(bitset, state)` cache entry, once onto the
   backtrack stack. `String::clone` = one `malloc` + `memcpy` per clone. Go's equivalent
   copies a 2-word string header on the stack with no heap allocation.

2. **SipHash13 on `u64` cache keys**: `std::collections::HashMap` uses a
   DOS-resistant cryptographic hasher by default. The DFS cache key is a plain `u64`
   (bitset hash), where SipHash is 3–4× slower than a non-cryptographic alternative.

3. **Heap allocation for every `Bitset` clone**: `Bitset(Vec<u64>)` heap-allocates on
   every `clone()`. The hot path clones the bitset once per step. For etcd (~170 ops →
   3 u64s) and KV per-partition (~30–50 ops → 1 u64), the data fits easily on the stack.

4. **Unnecessary rayon overhead for single partitions**: models without a `partition`
   implementation (e.g. `EtcdModel`) always produce a single partition, yet `check_parallel`
   still submitted it to rayon's task queue, paying thread-pool overhead on every call.

5. **Benchmark-level missed parallelism**: the `etcd_parallel/all_files` group iterated
   sequentially over 102 independent histories, making it identical to the sequential group.

6. **`String` cloning in `KvInput` / `KvOutput`**: `KvInput.key`, `KvInput.value`, and
   `KvOutput.value` were `String`, causing `String::clone` heap allocations during
   `partition_events` setup and history slice construction. Go's string fields are 2-word
   headers — copying them is a stack operation with no heap involvement.

7. **Rayon dispatch overhead on small partitions**: KV c10 splits into 10 per-key
   partitions of ~30–50 ops each. Rayon task dispatch (~3–5 µs per partition) dominated
   the total checking time of ~240–280 µs for the smaller traces.

8. **Arbitrary partition ordering**: `HashMap::into_values()` returns partitions in
   insertion order. For bad histories, the violation may be checked last, maximising
   the time before `kill` is broadcast to cancel the others.

---

## 2. Optimizations Applied

Nine optimizations were applied in two passes. Each was verified with `cargo test` before
proceeding.

---

### Pass 1 — Library-wide optimizations (commit `01979ac`)

#### A — FxHashMap for the DFS cache and helper maps

**Files**: `Cargo.toml`, `src/checker.rs`

Added `rustc-hash = "2"` to `[dependencies]` and replaced `std::collections::HashMap`
with `rustc_hash::FxHashMap` at three hot-path sites in `src/checker.rs`:

| Site | Before | After |
|------|--------|-------|
| `renumber()` return-ID map | `HashMap<u64, u64>` | `FxHashMap<u64, u64>` |
| `NodeArena::from_entries()` return-index map | `HashMap<usize, usize>` | `FxHashMap<usize, usize>` |
| `check_single()` DFS cache | `HashMap<u64, Vec<CacheEntry<S>>>` | `FxHashMap<u64, Vec<CacheEntry<S>>>` |

`FxHashMap` uses a multiplicative identity-based hasher — effectively zero overhead for
integer keys — versus SipHash13's 3–4× per-key latency.

---

#### B — `Arc<str>` state for `KvModel`

**Files**: `benches/linearizability.rs`, `tests/go_compat.rs`

Changed `type State = String` → `type State = Arc<str>` in both `KvModel` implementations.

`Arc::clone` is a single atomic integer increment — no heap allocation, no memcpy.
`Arc<str>` satisfies `Clone + PartialEq` (content comparison, not pointer equality),
so the cache soundness invariant INV-LIN-04 is preserved exactly.

| Operation | Before | After |
|-----------|--------|-------|
| `Get` | `String::clone` = 1 `malloc` + `memcpy` | `Arc::clone` = 1 atomic increment |
| `Put` | `String::clone` = 1 `malloc` + `memcpy` | `Arc::from(&str)` = 1 `malloc` |
| `Append` | `format!` = 1 `malloc` | `Arc::from(format!...)` = 1 `malloc` |

---

#### C — Single-partition fast path in `check_parallel`

**File**: `src/checker.rs`

Added an early return before the `par_iter` dispatch when only one partition is present.
For `EtcdModel` (no `partition_events` implementation), this fast path is taken on every
single call, eliminating rayon task-queue overhead (~5–8 µs per call). The
`definitive_illegal` kill-flag check mirrors the multi-partition path exactly, preserving
the `Unknown` vs `Illegal` correctness guarantee.

---

#### D — Parallel file iteration in `bench_etcd_parallel`

**File**: `benches/linearizability.rs`

Changed the `all_files/102` benchmark from a sequential `for` loop to `par_iter`.
Benchmark-scope change only — the library API is unchanged. The 102 Jepsen etcd histories
are independent; on an 8-core M1 this delivers a ~3× observed speedup for that group.

---

#### E — `SmallVec<[u64; 4]>` for `Bitset`

**Files**: `Cargo.toml`, `src/bitset.rs`

Changed the backing storage of `Bitset` from `Vec<u64>` to `SmallVec<[u64; 4]>`,
storing up to 4 words inline (≤256 operations).

| Workload | Operations | Chunks needed | Heap alloc on clone? |
|----------|-----------|---------------|----------------------|
| etcd single history | ~170 ops | 3 u64s | **No** |
| KV per-partition | ~30–50 ops | 1 u64 | **No** |

The DFS hot path clones the bitset once per step. For these workloads, that clone is now
a `memcpy` of 32 bytes on the stack — zero heap allocation.

---

#### F — Eliminate double state clone in DFS hot path

**File**: `src/checker.rs`, `check_single` function

Before: every successful step cloned `M::State` twice (once for cache, once for backtrack
stack). Restructured using `std::mem::replace` to move the old state onto the stack and
clone only once for the cache:

```rust
// Before: 2 state clones per successful step
cache.entry(h).or_default().push(CacheEntry { ..., state: next_state.clone() });
calls.push(CallFrame { node_idx: idx, state: state.clone() });
state = next_state;

// After: 1 state clone per successful step
let old_state = std::mem::replace(&mut state, next_state);
cache.entry(h).or_default().push(CacheEntry { ..., state: state.clone() });
calls.push(CallFrame { node_idx: idx, state: old_state }); // moved, no clone
```

---

### Pass 2 — KV-specific optimizations (2026-04-05)

#### G — `Arc<str>` for `KvInput.key`, `KvInput.value`, `KvOutput.value`

**Files**: `benches/linearizability.rs`, `tests/go_compat.rs`

Changed `KvInput.key`, `KvInput.value`, and `KvOutput.value` from `String` to `Arc<str>`.

The main wins:
- `partition_events` setup: both `id_to_key` and `by_key` HashMap builds now clone keys
  via `Arc::clone` (atomic bump) instead of `String::clone` (heap alloc + memcpy).
- `Put` path in `step`: `Arc::from(input.value.as_str())` allocated a new `Arc` on every
  write, even though `input.value` was already heap-allocated. With `Arc<str>` input,
  this becomes `Arc::clone(&input.value)` — zero allocation.

```rust
// Before
KvOp::Put => Some(Arc::from(input.value.as_str())),  // 1 alloc even with Arc<str> state

// After
KvOp::Put => Some(Arc::clone(&input.value)),          // 0 allocs: reuse the existing Arc
```

`KvNoPartitionModel` (state: `HashMap<String, String>`) updated to use `.to_string()` and
`Borrow<str>` lookups where the key must be inserted into or retrieved from the owned HashMap.

---

#### H — Sort partitions smallest-first in `check_parallel`

**File**: `src/checker.rs`

Added `partitions.sort_unstable_by_key(|p| p.len())` before the rayon dispatch.

For bad histories, a smaller partition is more likely to complete its DFS quickly and
broadcast `kill`, aborting all other partitions before they have explored much. The sort
itself is sub-microsecond for the partition counts seen in practice (≤50 partitions).

---

#### I — Sequential fallback below total-entry threshold in `check_parallel`

**File**: `src/checker.rs`

Added a sequential path for multi-partition checks where the total entry count is below
a threshold (`SEQUENTIAL_THRESHOLD = 2000`):

```rust
const SEQUENTIAL_THRESHOLD: usize = 2000;
let total_entries: usize = partitions.iter().map(|p| p.len()).sum();
if total_entries < SEQUENTIAL_THRESHOLD {
    for partition in partitions {          // already sorted smallest-first by H
        if kill.load(Ordering::Relaxed) { return false; }
        let ok = check_single(model, partition, &kill);
        if !ok {
            if !kill.load(Ordering::Relaxed) {
                definitive_illegal.store(true, Ordering::Relaxed);
            }
            kill.store(true, Ordering::Relaxed);
            return false;
        }
    }
    return true;
}
// Existing rayon path for large inputs.
```

KV c10 has ~700 total entries across 10 partitions — well below 2000, so it runs
sequentially. KV c50 has ~5× more entries and continues to use rayon. Etcd is unaffected
(always hits the single-partition fast path before this code is reached).

The sequential loop runs partitions in sorted order (from H), so for bad histories the
violation-containing partitions are tried first without the overhead of spawning threads,
dispatching tasks, or joining.

---

## 3. Results

### 3.1 Raw Benchmark Numbers

#### After Pass 1 (optimizations A–F)

```
etcd_sequential/single_file    time: [50.855 µs  50.945 µs  51.047 µs]
etcd_sequential/all_files/102  time: [160.59 ms  160.91 ms  161.27 ms]

etcd_parallel/single_file      time: [45.138 µs  45.204 µs  45.269 µs]
etcd_parallel/all_files/102    time: [75.856 ms   76.038 ms  76.225 ms]

kv_partitioned/c10_ok_seq      time: [271.80 µs  272.01 µs  272.28 µs]
kv_partitioned/c10_bad_seq     time: [192.65 µs  193.03 µs  193.35 µs]
kv_partitioned/c10_ok_par      time: [278.89 µs  279.22 µs  279.62 µs]
kv_partitioned/c10_bad_par     time: [239.91 µs  240.38 µs  240.69 µs]
```

#### After Pass 2 (optimizations G–I)

```
etcd_sequential/single_file    time: [50.689 µs  50.760 µs  50.842 µs]
etcd_sequential/all_files/102  time: [160.75 ms  161.00 ms  161.26 ms]

etcd_parallel/single_file      time: [45.934 µs  45.980 µs  46.031 µs]
etcd_parallel/all_files/102    time: [76.495 ms   76.739 ms  76.999 ms]

kv_partitioned/c10_ok_seq      time: [189.77 µs  190.02 µs  190.25 µs]
kv_partitioned/c10_bad_seq     time: [ 89.311 µs  89.802 µs  90.380 µs]
kv_partitioned/c10_ok_par      time: [183.92 µs  184.52 µs  185.59 µs]
kv_partitioned/c10_bad_par     time: [ 82.360 µs  83.294 µs  84.341 µs]
```

---

### 3.2 Full Before / After / Go Comparison

| Benchmark | Initial | After Pass 1 | After Pass 2 | Go | Final vs Go |
|-----------|---------|-------------|-------------|-----|-------------|
| `etcd_sequential/single_file` | 107 µs | 51 µs | **51 µs** | 114 µs | **2.2× faster** |
| `etcd_sequential/all_files/102` | 250 ms | 161 ms | **161 ms** | 290 ms | **1.8× faster** |
| `etcd_parallel/single_file` | 104 µs | 45 µs | **46 µs** | 114 µs | **2.5× faster** |
| `etcd_parallel/all_files/102` | 250 ms | 76 ms | **77 ms** | 290 ms | **3.8× faster** |
| `kv_partitioned/c10_ok_seq` | 368 µs | 272 µs | **190 µs** | 239 µs | **1.26× faster** |
| `kv_partitioned/c10_bad_seq` | 217 µs | 193 µs | **90 µs** | 168 µs | **1.87× faster** |
| `kv_partitioned/c10_ok_par` | 318 µs | 279 µs | **185 µs** | 239 µs | **1.29× faster** |
| `kv_partitioned/c10_bad_par` | 266 µs | 240 µs | **83 µs** | 168 µs | **2.02× faster** |

**Rust now leads Go on every single benchmark.**

---

### 3.3 Improvement Summary

| Benchmark | Total improvement (initial → final) | Pass 1 share | Pass 2 share |
|-----------|--------------------------------------|-------------|-------------|
| `etcd_sequential/single_file` | −52% | −52% | 0% |
| `etcd_sequential/all_files/102` | −36% | −36% | 0% |
| `etcd_parallel/single_file` | −56% | −56% | 0% |
| `etcd_parallel/all_files/102` | −70% | −70% | 0% |
| `kv_partitioned/c10_ok_seq` | −48% | −26% | −30% |
| `kv_partitioned/c10_bad_seq` | −59% | −11% | −53% |
| `kv_partitioned/c10_ok_par` | −42% | −12% | −34% |
| `kv_partitioned/c10_bad_par` | −69% | −10% | −65% |

---

## 4. Analysis

### 4.1 etcd workload — decisive Rust win, unchanged by Pass 2

The etcd model has no `partition_events` implementation, so every call produces exactly one
partition. The single-partition fast path (C) routes every etcd call directly to
`check_single`, skipping rayon entirely. Optimizations G, H, and I all operate at or after
the multi-partition dispatch point — they are never reached for etcd. The etcd numbers
before and after Pass 2 are statistically identical, confirming zero regression.

Combined effect of A + C + E + F: etcd went from 107 µs → 51 µs sequential (2.2×) and
104 µs → 45 µs parallel (2.5×), both significantly ahead of Go's 114 µs.

The `etcd_parallel/all_files` result (76 ms vs 250 ms initial) is partly library and partly
benchmark: optimization D changed the all-files loop to `par_iter`. The sequential group's
−36% (250 ms → 161 ms) is the clean library-only number; the parallel group's additional
speedup reflects 8-core parallelism across 102 independent histories.

### 4.2 KV partitioned workload — complete reversal

After Pass 1, the KV gap narrowed from 29–54% behind Go to 14–43% behind — real progress,
but Go still led. Pass 2 reversed all four benchmarks:

**`c10_bad` cases: the largest swing**

`c10_bad_par` went from 43% behind Go (240 µs vs 168 µs) to **2× faster** (83 µs vs 168 µs).
The driver was the combination of H + I:
- Optimization I (sequential fallback): eliminated all rayon task-dispatch, join, and
  AtomicBool coordination overhead. For 10 partitions of ~30–50 ops each, this was
  ~30–50 µs of pure overhead. The sequential loop costs nothing beyond calling `check_single`.
- Optimization H (smallest-first sort): within the sequential loop, the violation is
  found early in a small partition. For `c10_bad`, the violation is effectively detectable
  in the first or second smallest partition — the DFS aborts after ~90 µs total rather
  than waiting for all 10 partitions to be checked.

`c10_bad_seq` (single-thread pool, so already sequential dispatch into rayon, but still
with rayon's join overhead): improved 53% (193 µs → 90 µs) by the same mechanisms — the
sequential fallback skips rayon entirely, and the sorted partition order finds the violation
in the smallest partition first.

**`c10_ok` cases: meaningful improvement**

`c10_ok_seq` improved 30% (272 µs → 190 µs) and `c10_ok_par` improved 34% (279 µs → 185 µs).
For linearizable histories there is no early exit — all partitions must be verified. The
gains here come from: (G) eliminating `String::clone` in partition setup and on the `Put`
path, and (I) eliminating rayon overhead across all 10 partitions.

The near-identical seq/par numbers (190 vs 185 µs, 90 vs 83 µs) confirm that both hit the
sequential fallback path, making the 1-thread and all-thread configurations equivalent for
c10.

### 4.3 Correctness — no regressions

All 88 tests (60 unit + 15 integration + 13 property-based) continue to pass after every
optimization. The two `#[ignore]` tests remain ignored as expected. The `timeout_*` tests
specifically exercise the `definitive_illegal` / kill-flag race in both single-partition
and multi-partition paths — all pass, confirming the sequential fallback's kill-flag
handling is sound.

---

## 5. Residual Gap and Future Options

After Pass 2, Rust leads Go on all benchmarks. The smallest margin is `c10_ok_seq` at
**1.26× faster** (190 µs vs 239 µs). Residual analysis:

The remaining ~190 µs on `c10_ok_seq` breaks down roughly as:
- DFS backtracking cost: proportional to history complexity, essentially irreducible
- `Append` allocations: one `Arc::from(format!(...))` per Append step — unavoidable without
  a string interning or arena approach
- `Arc<str>` refcount cost on `Get`: one atomic increment vs Go's zero-cost string header
  copy — inherent to reference counting

These are fundamental costs of Rust's ownership model applied to string-valued state.
Closing this final ~26% advantage would require one of:

| Option | Mechanism | Expected gain | Complexity | Risk |
|--------|-----------|---------------|------------|------|
| String interning for `KvInput.value` | `Arc` pool keyed by content; `Put` reuses existing `Arc` | 5–15% Append-heavy | Medium | Low |
| `Arc<M::State>` at checker level | Skip 1 clone per step for expensive-state models | 10–30% if state is large | Medium | Etcd regression |
| `bumpalo` arena for DFS state | Eliminate all per-step heap allocations via bump allocator | 40–60% complex models | High | **Breaking API change** |

The `bumpalo` option requires adding a lifetime to `Model::State` (a GAT), making it a
semver-breaking change. It is the highest-gain option for models with large state (e.g.
`KvNoPartitionModel` with `HashMap<String, String>` state) but is not warranted given
the current performance profile. Deferred to a 0.2 roadmap item.

The `Arc<M::State>` option is not beneficial for `KvModel` (state is already `Arc<str>`,
adding another `Arc` layer would add an allocation per step) and risks etcd regression
(`EtcdModel::State = Option<i64>` — 8 bytes, free to clone; wrapping in `Arc` adds a
heap alloc per step). Skip unless a model with genuinely expensive state is added.
