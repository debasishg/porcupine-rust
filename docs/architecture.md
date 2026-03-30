# porcupine-rust — Architecture

A Rust port of [porcupine](https://github.com/anishathalye/porcupine), a fast linearizability checker for concurrent and distributed systems.

---

## (a) Component Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                          Public API (lib.rs)                         │
│                                                                      │
│   check_operations(model, history)    check_events(model, history)  │
└───────────────────────────┬──────────────────────┬──────────────────┘
                            │                      │
                            ▼                      ▼
┌───────────────────────────────────────────────────────────────────┐
│                       checker.rs                                   │
│                                                                    │
│  ┌──────────────┐   ┌───────────────────┐   ┌──────────────────┐ │
│  │ make_entries │   │ convert_entries / │   │ assert_well_     │ │
│  │              │   │ renumber          │   │ formed! (INV-    │ │
│  │ Op[] → Entry │   │                   │   │ HIST-01)         │ │
│  │ (call+return │   │ Event[] → Entry[] │   │                  │ │
│  │  per op,     │   │ (re-index ids,    │   │ assert_partition_│ │
│  │  time-sorted)│   │  use pos as time) │   │ independent!     │ │
│  └──────┬───────┘   └────────┬──────────┘   │ (INV-LIN-03)    │ │
│         └──────────────┬─────┘              └──────────────────┘ │
│                        ▼                                           │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │                   NodeArena<I, O>                            │  │
│  │                                                             │  │
│  │  Vec<Node> with sentinel HEAD at index 0                    │  │
│  │                                                             │  │
│  │  Node { value, match_idx, id, prev, next }                  │  │
│  │                                                             │  │
│  │  Sentinel ──► Node₁ ◄──► Node₂ ◄──► … ◄──► Nodeₙ          │  │
│  │   (idx 0)    (call)  (return)                               │  │
│  │                                                             │  │
│  │  lift(call_idx)   — unlink call + matched return            │  │
│  │  unlift(call_idx) — re-link both back in original position  │  │
│  └─────────────────────────────┬───────────────────────────────┘  │
│                                │                                   │
│             ┌──────────────────▼──────────────────┐               │
│             │          check_single (DFS)          │               │
│             │                                      │               │
│             │  cursor ──► walk live list           │               │
│             │                                      │               │
│             │  Call node?                          │               │
│             │    model.step(state, in, out)        │               │
│             │    ├─ None  → advance cursor         │               │
│             │    └─ Some  → cache_contains?        │               │
│             │               ├─ hit  → skip         │               │
│             │               └─ miss → push frame,  │               │
│             │                         lift, restart│               │
│             │                                      │               │
│             │  Return node? → backtrack            │               │
│             │    pop frame, unlift, advance        │               │
│             │                                      │               │
│             │  None (end of list) → return true    │               │
│             └──────────────┬───────────────────────┘               │
│                            │                                       │
│             ┌──────────────▼───────────────────────┐              │
│             │   DFS Cache                           │              │
│             │                                       │              │
│             │   HashMap<u64, Vec<CacheEntry<S>>>    │              │
│             │                                       │              │
│             │   key   = bitset.hash()               │              │
│             │   value = Vec<{ Bitset, State }>      │              │
│             │                                       │              │
│             │   Prunes duplicate (bitset, state)    │              │
│             │   branches (INV-LIN-04)               │              │
│             └───────────────────────────────────────┘              │
│                                                                    │
│  ┌─────────────────────────────────────────────────────────────┐  │
│  │  check_parallel                                              │  │
│  │                                                             │  │
│  │  for each partition:                                        │  │
│  │    check_single(model, partition_entries, &kill_flag)       │  │
│  │    if Illegal → set kill flag, return early                 │  │
│  └─────────────────────────────────────────────────────────────┘  │
└───────────────────────────────────────────────────────────────────┘
           │                        │
           ▼                        ▼
┌──────────────────┐     ┌────────────────────────────────────┐
│   bitset.rs      │     │   model.rs                         │
│                  │     │                                     │
│  Bitset(Vec<u64>)│     │  trait Model {                     │
│                  │     │    type State: Clone + PartialEq;  │
│  set(pos)        │     │    type Input: Clone;              │
│  clear(pos)      │     │    type Output: Clone;             │
│  popcnt()        │     │    fn init() → State               │
│  hash()          │     │    fn step(s, i, o) → Option<S>   │
│  equals(other)   │     │    fn partition(…) → Option<…>    │
│  clone()         │     │  }                                 │
└──────────────────┘     └────────────────────────────────────┘

┌────────────────────────────────────────────────────────────────────┐
│  types.rs                                                          │
│                                                                    │
│  Operation<I,O>  { client_id, input, call, output, return_time }  │
│  Event<I,O>      { client_id, kind, input, output, id }           │
│  CheckResult     { Ok | Illegal | Unknown }                        │
│  LinearizationInfo { partitions }   (stub — not yet populated)    │
└────────────────────────────────────────────────────────────────────┘

┌────────────────────────────────────────────────────────────────────┐
│  invariants.rs  (debug_assert! macros, INV-* keyed)               │
│                                                                    │
│  assert_well_formed!         INV-HIST-01                          │
│  assert_minimal_call!        INV-HIST-03  (structural in DFS)     │
│  assert_partition_independent! INV-LIN-03                         │
│  assert_cache_sound!         INV-LIN-04   (structural in cache)   │
└────────────────────────────────────────────────────────────────────┘
```

---

## (b) Design Considerations

### Generic Model trait — zero-cost abstraction

The `Model` trait is monomorphised at compile time. There is no dynamic dispatch anywhere in the hot path. Users define a concrete `State`, `Input`, and `Output` type; the compiler generates a fully inlined DFS loop specialised to those types. This mirrors the generic type parameters in the Go original but with stronger compile-time guarantees.

The `State: Clone + PartialEq` bounds are the minimum required to support the cache (`clone` to save pre-step state on the call stack; `PartialEq` to test cache equality). No `Hash` or `Ord` bounds are imposed on the model.

### Two history representations

The public API accepts either:

- **`Operation<I,O>`** — a completed operation with wall-clock call/return timestamps. Most users of testing frameworks produce this format.
- **`Event<I,O>`** — a raw call-then-return event stream (useful when reading logs from a system under test that records events as they arrive, before pairing them).

The `Event` path renumbers IDs to be contiguous (`renumber`) and uses event position as a logical timestamp (`convert_entries`). Both paths converge on the same `Vec<Entry<I,O>>` before the DFS.

### Index-based arena — safe doubly-linked list without `unsafe`

The DFS needs a doubly-linked list so that `lift` and `unlift` are O(1). Rust's ownership model makes intrusive pointer-based linked lists either `unsafe` or cumbersome (`Rc<RefCell<Node>>`). The solution is an index-based **`NodeArena`**: nodes live in a `Vec<Node>`, and `prev`/`next` fields are `usize` indices rather than pointers.

This gives:
- Zero `unsafe` code.
- No reference cycles.
- Cache-friendly linear allocation: all nodes allocated in a single `Vec::with_capacity(n + 1)` call.
- `lift` and `unlift` are six index writes each — no allocations, no heap fragmentation.

The sentinel `HEAD` node at index 0 eliminates all special-casing for "is this the first node?" in the link/unlink logic.

### Bitset for linearized set

Rather than a `HashSet<usize>` to track which operations have been linearized, the DFS uses a compact `Bitset(Vec<u64>)`. For a history of `n` operations, the bitset needs `⌈n/64⌉` words. Typical concurrency tests involve tens to a few hundred operations, so the bitset fits in 1–4 cache lines.

Using the bitset instead of a hash set:
- `set` / `clear` — single bit-op, no hashing.
- `hash` — XOR over all words, O(⌈n/64⌉).
- `equals` — slice comparison, LLVM-vectorisable.
- `popcnt` — uses `u64::count_ones()`, which compiles to a single `POPCNT` instruction on x86-64.

### DFS cache keyed by `(bitset_hash, state)`

The cache prunes duplicate DFS branches: if the same set of linearized operations leads to the same model state, there is no point re-exploring that subtree. The key is a `u64` XOR hash of the bitset (fast to compute, no heap allocation), with collisions resolved by a `Vec<CacheEntry<S>>` at each bucket (almost always length 1 in practice).

Storing `(Bitset, State)` per entry rather than just the state is necessary to distinguish different linearization orderings that happen to reach the same state through different paths — the bitset identifies which operations have been consumed.

### P-compositionality — partition-independent checking

If the model implements `partition`, `check_operations` splits the history into independent sub-histories and calls `check_single` on each. An `AtomicBool` kill flag is shared across all partition checks: as soon as one returns `Illegal`, the flag is set and subsequent partitions short-circuit their DFS loop.

This mirrors the `checkParallel` function in the Go original and enables dramatic speedups for models like key-value stores where per-key sub-histories are independent.

### Stability vs. the Go original

The Go implementation uses `goroutines` to run partition checks in parallel. In this Rust port, `check_parallel` is currently **sequential**. The `AtomicBool` kill flag is already in place as infrastructure; enabling true parallelism requires only wrapping each `check_single` call in a Rayon parallel iterator (see §d).

---

## (c) Optimizations

### Implemented

#### Compact bitset (storage + cache locality)

`Bitset(Vec<u64>)` stores one bit per operation. For `n = 64` ops (a large history), the entire bitset is a single 8-byte word — the whole linearized set fits in a register. Even for `n = 256`, it is four 64-bit words, a single cache line. Compare to `HashSet<usize>`, which allocates heap memory and involves pointer chasing.

#### Single-allocation node arena (layout optimization)

`NodeArena::from_entries` allocates exactly `n + 1` nodes in one `Vec::with_capacity(n + 1)` call before populating them. The DFS then accesses these nodes by integer index with no further allocation. This keeps all node data contiguous in memory, making sequential list traversal cache-friendly (adjacent nodes are at predictable offsets in the same allocation).

#### Pre-sized data structures (allocation avoidance)

- `make_entries` uses `Vec::with_capacity(ops.len() * 2)` — no reallocation during entry construction.
- `NodeArena::from_entries` pre-allocates `n + 1` slots.
- `calls` (the DFS frame stack) grows on demand but never reallocates during backtracking since `Vec::pop` does not release capacity.
- `return_idx` HashMap is populated in one pass and discarded after arena construction.

#### In-place bitset mutation (no clone on backtrack)

The `linearized` bitset is mutated in-place with `set(op_id)` on linearization and `clear(op_id)` on backtrack — no clone needed on the backtrack path. A clone is only made when inserting a new entry into the DFS cache (`new_linearized.clone()` on the forward path). This means the common backtrack path (pop frame, clear bit, unlift) is allocation-free.

#### `POPCNT` hardware instruction via `count_ones()`

`Bitset::popcnt` calls `u64::count_ones()` on each word. LLVM (and `rustc`) compiles this to a single `POPCNT` instruction on x86-64 and ARM64, making population count O(⌈n/64⌉) with a very small constant.

#### Cache hash computation (XOR folding)

The bitset hash is `popcnt ⊕ w₀ ⊕ w₁ ⊕ … ⊕ wₖ`. This is trivially computed in a single pass over the bitset words with no heap allocation and produces a good spread for typical histories (most bitsets differ by one bit per DFS step). The `popcnt` seed prevents anagram collisions (two bitsets with the same set bits in different positions that happen to XOR-cancel each other).

#### Kill flag with `Ordering::Relaxed`

The `AtomicBool` kill flag uses `Relaxed` ordering for both load and store. Since the flag is monotone (false → true, never reset) and we only care that eventually all threads observe `true`, no memory ordering synchronisation is needed. `Relaxed` atomic ops compile to plain loads/stores on x86-64 (no `MFENCE`/`LOCK` prefix).

#### Stable sort tie-breaking (calls before returns)

In `make_entries`, the sort comparator places a call event before a return event at the same timestamp. This is semantically required (a call at time t must be ordered before a return at the same time t to correctly model instantaneous operations), and using `sort_by` (stable when keys differ) with a deterministic tiebreak ensures reproducible DFS traversal order.

### Not yet implemented — future opportunities

#### Parallel partition checking (Rayon)

`check_parallel` is currently sequential despite having an `AtomicBool` kill flag ready for concurrent use. Adding `rayon::iter::ParallelIterator` on the partition loop would enable true multi-core execution. The blocker is the `M::State: Send` bound that Rayon requires — this is not currently imposed by the `Model` trait. Plan: add an optional `parallel` feature that requires `M::State: Send + Sync` and uses `rayon::scope` instead of a `for` loop.

#### Cache locality for DFS frame stack

The `calls: Vec<CallFrame<S>>` stack stores a clone of `M::State` on every forward step. For large or heap-allocated state types (e.g. a `HashMap`), each push allocates. A possible optimization is to use a persistent/functional state representation (e.g. a persistent trie or a copy-on-write structure) so that state snapshots share structure. This is model-specific and cannot be done generically without changing the `Model` trait contract.

#### Prefetching node data during list traversal

The DFS walks the linked list node-by-node via `cursor = arena.nodes[idx].next`. Since nodes are stored contiguously in the arena and accessed roughly in index order (especially early in the DFS), sequential access patterns already benefit from hardware prefetchers. However, after many `lift`/`unlift` operations the live list no longer follows the physical node order. Explicit `std::intrinsics::prefetch_read_data` on `nodes[cursor.next]` during traversal could reduce cache miss stalls for large histories. Not yet implemented.

#### Bitset equality fast-path

`Bitset::equals` compares two `Vec<u64>` slices element-by-element. LLVM typically auto-vectorises this with SIMD (SSE2/AVX2), but an explicit `SIMD` implementation using `std::simd` (stabilising in Rust) would guarantee vectorisation and could be 2–4× faster for large histories (`n > 256`). Not yet implemented.

#### Cache eviction / bounded memory

The DFS cache (`HashMap<u64, Vec<CacheEntry<S>>>`) grows unbounded for the duration of a single `check_single` call. For very long histories this could consume significant memory. A bounded LRU cache (e.g. evicting least-recently-used entries) would cap memory use at the cost of potentially re-exploring some subtrees. Not yet implemented; the Go original also uses an unbounded cache.

#### `check_operations_timeout`

A timeout variant of the public API (`check_operations_timeout(model, history, duration) -> CheckResult`) is not yet implemented. The `kill` flag infrastructure is in place: a background thread would set it after the deadline, causing `check_single` to return `false` (which surfaces as `CheckResult::Unknown`). This is needed for production use against adversarial or very large histories.

---

## (d) Concurrency

### Current state — sequential with kill-flag infrastructure

All partition checks run sequentially in `check_parallel`. The `AtomicBool` kill flag (`std::sync::atomic::AtomicBool`) is threadsafe and shared by reference, but currently no actual threads are spawned.

```rust
// Current: sequential loop
for partition in partitions {
    if !check_single(model, partition, &kill) {
        kill.store(true, Ordering::Relaxed);
        return CheckResult::Illegal;
    }
}
```

### Planned — parallel partition checking via Rayon

The idiomatic Rust approach is to replace the sequential loop with a Rayon parallel iterator:

```rust
// Planned (requires M::State: Send)
partitions
    .into_par_iter()
    .find_any(|partition| !check_single(model, partition, &kill))
    .map_or(CheckResult::Ok, |_| {
        kill.store(true, Ordering::Relaxed);
        CheckResult::Illegal
    })
```

This would be gated behind an optional `parallel` Cargo feature to avoid imposing `Send` on all users:

```toml
[features]
parallel = ["rayon"]
```

The `Model` trait would gain a blanket impl or a separate `ParallelModel` marker trait requiring `State: Send + Sync`.

### Why parallelism matters

P-compositionality means partition sub-histories are completely independent — there is no shared mutable state between partition checks other than the kill flag. This is an embarrassingly parallel workload. For a key-value store with `k` independent keys, parallel checking scales linearly with `k` up to the number of available cores.

### Thread safety of the current design

- `NodeArena` and `Bitset` are not `Send` — each DFS instance owns its arena exclusively. No sharing is needed because each partition check creates its own arena from its own entry slice.
- `AtomicBool` kill flag: the flag is passed by shared reference (`&AtomicBool`) and is the only cross-thread communication.
- `Model` itself is accessed read-only (`&M` reference in `check_single`). As long as `M: Sync`, this is safe with multiple threads.

---

## (e) Error Handling and Correctness Contracts

### `debug_assert!` macros vs. runtime panics

All `INV-*` assertions in `invariants.rs` use `debug_assert!`, which is compiled out in release builds (`--release`). The contract is:

- **Debug builds** (`cargo test`, `cargo build`): invariant violations panic immediately with a descriptive message and the `INV-*` ID.
- **Release builds**: no overhead. The checker assumes well-formed inputs (the caller's responsibility).

This follows the Rust convention for precondition checking in library code: panic in debug, trust the caller in release.

### No `unsafe` code

The entire codebase contains zero `unsafe` blocks. The index-based arena design is specifically chosen to avoid the `unsafe` that a raw-pointer linked list would require.

### `LinearizationInfo` — stub for future visualization

`LinearizationInfo` exists in `types.rs` and is part of the public API surface, but its `partitions` field is never populated (`Default::default()`). The Go original uses this structure to produce an HTML visualization of the partial linearization for debugging non-linearizable histories. Populating it requires threading linearization order information back out through `check_single` — planned but not yet implemented.

---

## (f) Module Summary

| Module | Visibility | Role |
|--------|-----------|------|
| `src/lib.rs` | public | Crate root; re-exports public API |
| `src/types.rs` | public | `Operation`, `Event`, `CheckResult`, `LinearizationInfo` |
| `src/model.rs` | public | `Model` trait |
| `src/checker.rs` | public | Entry points + full DFS implementation |
| `src/bitset.rs` | crate-private | Compact bitset for linearized set and cache key |
| `src/invariants.rs` | crate-private | `debug_assert!` macros keyed to `INV-*` IDs |

`bitset` and `invariants` are intentionally `pub(crate)`: they are implementation details of the checker and not part of the public API. Users interact only with `Model`, `Operation`/`Event`, and `CheckResult`.
