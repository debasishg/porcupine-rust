# porcupine-rust — Benchmark Improvements

> **Date**: 2026-04-05 | **Machine**: Apple M1 | **Rust**: stable | **Go**: 1.26.1

---

## 1. Motivation

The initial benchmark comparison (documented in `docs/benchmark_results.md`) revealed that
the Rust port was 29–54% **slower** than the original Go implementation on KV partitioned
workloads — the very class where porcupine claims its largest speedups.

| Workload | Rust (before) | Go | Rust/Go |
|----------|--------------|-----|---------|
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

---

## 2. Changes Made

Six optimizations were applied, ordered by impact-to-risk ratio and implemented
incrementally with `cargo test` verification after each step.

---

### A — FxHashMap for the DFS cache and helper maps

**Files**: `Cargo.toml`, `src/checker.rs`

Added `rustc-hash = "2"` to `[dependencies]` and replaced `std::collections::HashMap`
with `rustc_hash::FxHashMap` at three hot-path sites in `src/checker.rs`:

| Site | Before | After |
|------|--------|-------|
| `renumber()` return-ID map | `HashMap<u64, u64>` | `FxHashMap<u64, u64>` |
| `NodeArena::from_entries()` return-index map | `HashMap<usize, usize>` | `FxHashMap<usize, usize>` |
| `check_single()` DFS cache | `HashMap<u64, Vec<CacheEntry<S>>>` | `FxHashMap<u64, Vec<CacheEntry<S>>>` |

`FxHashMap` uses a multiplicative identity-based hasher — effectively zero overhead for
integer keys — versus SipHash13's 3–4× per-key latency. The `HashMap` import was removed
from the module-level imports (test modules use their own local imports and are not in
the hot path).

---

### B — `Arc<str>` state for `KvModel`

**Files**: `benches/linearizability.rs`, `tests/go_compat.rs`

Changed `type State = String` → `type State = Arc<str>` in both `KvModel` implementations.

```rust
// Before
fn init(&self) -> String { String::new() }
fn step(&self, state: &String, ...) -> Option<String> {
    match input.op {
        KvOp::Get    => if output.value == *state { Some(state.clone()) } else { None },
        KvOp::Put    => Some(input.value.clone()),
        KvOp::Append => Some(format!("{}{}", state, input.value)),
    }
}

// After
fn init(&self) -> Arc<str> { Arc::from("") }
fn step(&self, state: &Arc<str>, ...) -> Option<Arc<str>> {
    match input.op {
        KvOp::Get    => if output.value.as_str() == state.as_ref() {
                            Some(Arc::clone(state))  // atomic refcount bump, no alloc
                        } else { None },
        KvOp::Put    => Some(Arc::from(input.value.as_str())),
        KvOp::Append => Some(Arc::from(format!("{}{}", state, input.value).as_str())),
    }
}
```

`Arc::clone` is a single atomic integer increment — no heap allocation, no memcpy.
`Arc<str>` satisfies `Clone + PartialEq` (content comparison, not pointer equality),
so the cache soundness invariant INV-LIN-04 is preserved exactly. Library code in `src/`
is untouched.

| Operation | Before | After |
|-----------|--------|-------|
| `Get` | `String::clone` = 1 `malloc` + `memcpy` | `Arc::clone` = 1 atomic increment |
| `Put` | `String::clone` = 1 `malloc` + `memcpy` | `Arc::from(&str)` = 1 `malloc` |
| `Append` | `format!` = 1 `malloc` | `Arc::from(format!...)` = 1 `malloc` |

---

### C — Single-partition fast path in `check_parallel`

**File**: `src/checker.rs`

Added an early return before the `par_iter` dispatch when only one partition is present:

```rust
// Fast path: single partition avoids all rayon task-submission overhead.
// For models without partitioning (e.g. EtcdModel), this is always taken.
if partitions.len() == 1 {
    let ok = check_single(model, partitions.into_iter().next().unwrap(), &kill);
    if !ok {
        if !kill.load(Ordering::Relaxed) {
            definitive_illegal.store(true, Ordering::Relaxed);
        }
        kill.store(true, Ordering::Relaxed);
    }
    return ok;
}
```

The `definitive_illegal` logic mirrors the multi-partition rayon path exactly — the kill-flag
race-free check ensures `Unknown` is returned (not `Illegal`) when the timer fires before
the DFS completes. Two timeout tests (`timeout_short_duration_returns_unknown`,
`timeout_short_duration_events_returns_unknown`) confirmed the fix is sound.

For `EtcdModel` (no `partition_events` implementation), this fast path is taken on every
single call, eliminating rayon task-queue overhead (~5–8 µs per call at 107 µs baseline
= ~5–7% of total time).

---

### D — Parallel file iteration in `bench_etcd_parallel`

**File**: `benches/linearizability.rs`

Changed the `all_files/102` benchmark in `bench_etcd_parallel` from a sequential `for`
loop to a rayon `par_iter`:

```rust
// Before
b.iter(|| {
    for h in &all_histories {
        let _ = check_events(&EtcdModel, h, None);
    }
});

// After
b.iter(|| {
    all_histories.par_iter().for_each(|h| {
        let _ = check_events(&EtcdModel, h, None);
    });
});
```

Added `use rayon::prelude::*;` at the top of the bench file. This is a benchmark-scope
change only — the library API is unchanged. The 102 Jepsen etcd histories are independent,
making this embarrassingly parallel. On the M1's 8 cores, this delivers a ~3× observed
speedup (76 ms vs 161 ms sequential), and the parallel group now meaningfully diverges
from the sequential group.

---

### E — `SmallVec<[u64; 4]>` for `Bitset`

**Files**: `Cargo.toml`, `src/bitset.rs`

Added `smallvec = "1"` to `[dependencies]` and changed the backing storage of `Bitset`
from `Vec<u64>` to `SmallVec<[u64; 4]>`:

```rust
// Before
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitset(Vec<u64>);

impl Bitset {
    pub fn new(n: usize) -> Self {
        Bitset(vec![0u64; n.div_ceil(64)])
    }
}

// After
use smallvec::SmallVec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitset(SmallVec<[u64; 4]>);

impl Bitset {
    pub fn new(n: usize) -> Self {
        let mut data: SmallVec<[u64; 4]> = SmallVec::new();
        data.resize(n.div_ceil(64), 0u64);
        Bitset(data)
    }
}
```

`SmallVec<[u64; 4]>` stores up to 4 words inline (≤256 operations). Both typical workloads
fit entirely within the inline buffer:

| Workload | Operations | Chunks needed | Heap alloc? |
|----------|-----------|---------------|-------------|
| etcd single history | ~170 ops | 3 u64s | **No** |
| KV per-partition | ~30–50 ops | 1 u64 | **No** |

The DFS hot path clones the bitset once per step (`let mut new_linearized = linearized.clone()`).
For these workloads, that clone is now a `memcpy` of 32 bytes on the stack — zero heap allocation.
All methods (`set`, `clear`, `popcnt`, `hash`) use `self.0[idx]` and `&self.0`, which compile
identically because `SmallVec` implements `Deref<Target=[u64]>` and `IndexMut<usize>`.

---

### F — Eliminate double state clone in DFS hot path

**File**: `src/checker.rs`, `check_single` function

Before this change, every successful DFS step cloned `M::State` twice:
1. `next_state.clone()` → stored in the `(bitset, state)` cache entry
2. `state.clone()` → old state saved onto the backtrack stack; then `state = next_state`

Restructured using `std::mem::replace` to move the old state onto the stack (no clone)
and clone only once for the cache:

```rust
// Before: 2 state clones per successful step
cache.entry(h).or_default().push(CacheEntry { ..., state: next_state.clone() });
calls.push(CallFrame { node_idx: idx, state: state.clone() });
state = next_state;

// After: 1 state clone per successful step
let old_state = std::mem::replace(&mut state, next_state);
// state is now next_state; old_state is the pre-step state
cache.entry(h).or_default().push(CacheEntry { ..., state: state.clone() });
calls.push(CallFrame { node_idx: idx, state: old_state }); // moved, no clone
```

Correctness: `CallFrame::state` stores the pre-step state (`old_state`, moved).
`CacheEntry::state` stores the post-step state (`state` after the replace, cloned once).
The backtrack path (`state = frame.state`) is unaffected. INV-LIN-04 is preserved.

---

## 3. Results

### 3.1 New Benchmark Numbers (after all six optimizations)

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

### 3.2 Before vs After vs Go

| Benchmark | Before | After | Improvement | Go | Rust vs Go (after) |
|-----------|--------|-------|-------------|-----|-------------------|
| `etcd_sequential/single_file` | 107 µs | **51 µs** | −52% | 114 µs | **2.2× faster** |
| `etcd_sequential/all_files/102` | 250 ms | **161 ms** | −36% | 290 ms | **1.8× faster** |
| `etcd_parallel/single_file` | 104 µs | **45 µs** | −56% | 114 µs | **2.5× faster** |
| `etcd_parallel/all_files/102` | 250 ms | **76 ms** | −70% | 290 ms | **3.8× faster** |
| `kv_partitioned/c10_ok_seq` | 368 µs | **272 µs** | −26% | 239 µs | 1.14× (Go 14% faster) |
| `kv_partitioned/c10_bad_seq` | 217 µs | **193 µs** | −11% | 168 µs | 1.15× (Go 15% faster) |
| `kv_partitioned/c10_ok_par` | 318 µs | **279 µs** | −12% | 239 µs | 1.17× (Go 17% faster) |
| `kv_partitioned/c10_bad_par` | 266 µs | **240 µs** | −10% | 168 µs | 1.43× (Go 43% faster) |

### 3.3 Criterion Change Report

Criterion reports percentage change relative to the previous stored baseline:

| Benchmark | Criterion Δ |
|-----------|-------------|
| `etcd_sequential/single_file` | −52.5% |
| `etcd_sequential/all_files/102` | −35.8% |
| `etcd_parallel/single_file` | −56.4% |
| `etcd_parallel/all_files/102` | −69.6% |
| `kv_partitioned/c10_ok_seq` | −25.3% |
| `kv_partitioned/c10_bad_seq` | −14.0% |
| `kv_partitioned/c10_ok_par` | −11.9% |
| `kv_partitioned/c10_bad_par` | −9.7% |

All changes are statistically significant (p = 0.00 < 0.05).

---

## 4. Analysis

### 4.1 etcd workload — decisive Rust win

The etcd model has no `partition_events` implementation, so every call to `check_events`
produces exactly one partition. Before this work, that single partition was dispatched
through rayon's `par_iter().any()` regardless — submitting a task, allocating an
`AtomicBool`, and joining, adding ~5–8 µs of overhead to a 107 µs call (~5–7%).

Optimization C eliminated this overhead entirely (single-partition fast path). Combined
with FxHashMap (A), SmallVec bitset (E), and the halved state-clone count (F), etcd went
from 107 µs → 51 µs sequential and from 104 µs → 45 µs parallel — a 2.2–2.5× lead
over Go's 114 µs.

The `etcd_parallel/all_files` result (76 ms vs 250 ms before, 290 ms Go) deserves
separate accounting: optimization D changed the benchmark to iterate over 102 histories
in parallel rather than sequentially. The 70% improvement on this group is largely from
the benchmark parallelism, not the library changes. The sequential group's −36% (250 ms
→ 161 ms) is the clean library-only number for the all-files workload.

### 4.2 KV partitioned workload — gap closed but not eliminated

The KV gap narrowed from 29–54% behind Go to 14–43% behind. The `Arc<str>` change (B)
was the biggest lever: `Get` operations (read-heavy workloads) went from one heap
allocation per step to zero. `Put` and `Append` each retain one allocation (unavoidable —
a new string value must be created).

The remaining gap has the same root cause as before, just smaller:
- `Arc::from(input.value.as_str())` for `Put` still allocates. Go's string assignment is
  a pointer copy.
- The per-key partitions in `c10` have ~30–50 ops each; rayon thread-pool overhead for
  10 partitions (~3–5 µs dispatch) is still a meaningful fraction of the total 240–280 µs.
- `c10_bad_par` is slowest relative to Go: a violation is found almost immediately in
  one partition; the other threads are cancelled, but cancellation + coordination overhead
  is not free.

### 4.3 Correctness — no regressions

All 88 tests (60 unit + 15 integration + 13 property-based) continue to pass. The 2
ignored tests remain ignored (expected: `kv_no_partition_10_clients` takes 60–90 s without
partitioning). The `timeout_short_duration_returns_unknown` tests specifically exercise the
`definitive_illegal` / timeout race introduced by optimization C — both pass, confirming
the kill-flag check in the single-partition fast path is sound.

---

## 5. Remaining Gap and Future Options

The KV workload (partitioned) still lags Go by 14–43%. Further options, in order of
expected impact vs. implementation complexity:

| Option | Mechanism | Expected gain | Complexity |
|--------|-----------|---------------|-----------|
| `Arc<str>` for `KvInput.key` / `KvInput.value` | Avoid `String` clone during `partition_events` pass | 5–10% KV | Low |
| Skip rayon dispatch when partitions are all tiny | Threshold on partition size, run sequentially if below | 5–15% KV par | Low |
| Cache-aware partition ordering | Sort partitions by size (small-first) so violations surface earlier | 5–20% KV bad | Low |
| `Arc<M::State>` at checker level | All models get free state clone; costs 1 `Arc::new` per step | 20–40% KV, regression for etcd | Medium |
| `bumpalo` arena for DFS state | Eliminate all per-state allocations; reset per check_single call | 40–60% complex models | High |

The etcd workload now runs 2–2.5× faster than Go, well ahead of the initial 7–16% margin.
The KV workload is closer but still behind — the gap is now primarily from `Put`/`Append`
allocations and rayon overhead on small partitions, not from the dominant `Get`-path cloning
that was fixed here.
