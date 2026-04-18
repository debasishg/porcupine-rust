# Linearizability Checking — Algorithm Walkthrough

This document explains the linearizability checking algorithm from first
principles.  For every step it shows the corresponding Rust implementation in
`src/checker.rs` and supporting modules.  It is meant to be read alongside
[architecture.md](architecture.md), which covers the component layout,
optimisations, and concurrency design.

---

## 1. What is linearizability?

A concurrent system is **linearizable** if every concurrent execution is
indistinguishable from some sequential execution that respects the real-time
ordering of operations.

Imagine multiple clients talking to a shared register at the same time.  Each
operation has a **call** (when the client sends the request) and a **return**
(when the client gets the response).  Between those two points the operation
"takes effect" at some instant — its **linearization point**.

A history is linearizable when we can assign a linearization point to every
operation such that:

1. Each linearization point falls between the operation's call and return.
2. The sequence of operations ordered by their linearization points is a valid
   sequential execution of the specification (the "model").

If no such assignment exists, the history is **not linearizable** — the system
violated its specification under concurrency.

### Why is checking hard?

The number of possible orderings is exponential in the number of concurrent
operations.  For `n` overlapping operations, there are up to `n!` candidate
linearization orders.  Linearizability checking is NP-complete in general, so
every practical checker uses pruning strategies to keep the search tractable.

---

## 2. The Model — sequential specification

Before we can check a history, we need a **model**: a description of how the
system should behave if operations happened one at a time.

A model is a state machine with three parts:

- **Initial state** — the state before any operation.
- **Step function** — given the current state and an (input, output) pair,
  returns the next state if the transition is valid, or `None` if it is
  impossible.
- **Partition function** (optional) — splits a history into independent
  sub-histories that can be checked separately (see Step 4).

### Rust implementation (`src/model.rs`)

```rust
pub trait Model {
    type State: Clone + PartialEq;
    type Input: Clone;
    type Output: Clone;

    fn init(&self) -> Self::State;
    fn step(&self, state: &Self::State, input: &Self::Input,
            output: &Self::Output) -> Option<Self::State>;
    fn partition(&self, history: &[Operation<Self::Input, Self::Output>])
        -> Option<Vec<Vec<usize>>>;
}
```

The trait is generic: users supply concrete types for `State`, `Input`, and
`Output`.  The compiler monomorphises the entire DFS loop for each model,
producing a specialised, fully-inlined checker with zero dynamic dispatch.

**Example — integer register:**

```text
State = i32 (register value, initially 0)

step(state, Write(v), _) → Some(v)        // always valid
step(state, Read,     v) → if v == state { Some(state) } else { None }
```

A read that returns a value different from the current state is rejected.

---

## 3. History representation — Operations and Events

The checker accepts histories in two formats:

### Operations

Each `Operation<I, O>` is a completed concurrent operation with wall-clock
timestamps:

| Field         | Meaning                            |
|---------------|------------------------------------|
| `client_id`   | Which client issued the operation  |
| `input`       | The request (e.g. `Write(5)`)      |
| `call`        | Timestamp when the operation was invoked |
| `output`      | The response (e.g. `Ok` or `3`)    |
| `return_time` | Timestamp when the response arrived |

### Events

Each `Event<I, O>` is a single call or return, useful when reading raw logs:

| Field       | Meaning                              |
|-------------|--------------------------------------|
| `client_id` | Client identifier                    |
| `kind`      | `Call` or `Return`                   |
| `input`     | Present for calls, `None` for returns |
| `output`    | Present for returns, `None` for calls |
| `id`        | Links a call to its matching return  |

### Rust implementation (`src/types.rs`)

Both types are defined as plain structs.  The event path goes through
`renumber` (makes IDs contiguous starting at 0) and `convert_entries` (uses
event position as logical timestamp).  Both paths converge into a single
internal `Vec<Entry<I, O>>` that the DFS operates on.

---

## 4. P-compositionality — divide and conquer

**P-compositionality** is a property of linearizability that allows a history
to be checked by partitioning it into independent sub-histories and checking
each one separately.  If the model's operations on different "keys" (or
partitions) do not interact, the history is linearizable if and only if every
partition is independently linearizable.

For a key-value store, operations on key `"x"` are independent of operations
on key `"y"`.  Instead of checking one history with all keys interleaved, we
split into per-key sub-histories.  This turns an exponential search over `n`
operations into multiple smaller searches over `n₁, n₂, …` operations — a
dramatic speedup.

### Rust implementation (`src/checker.rs`, `src/model.rs`)

If the model's `partition()` method returns `Some(groups)`, where each group is
a list of operation indices, the checker:

1. Validates that the partition is well-formed (INV-LIN-03: indices are
   disjoint and cover the whole history).
2. Builds a separate `Vec<Entry>` for each partition.
3. Checks all partitions via `check_parallel`, which dispatches them to rayon
   worker threads (or runs them sequentially for small inputs).

```rust
let partitions: Vec<Vec<Entry<M::Input, M::Output>>> =
    if let Some(parts) = model.partition(history) {
        assert_partition_independent!(parts);
        parts.iter().map(|indices| make_entries(&sub_history(indices))).collect()
    } else {
        vec![make_entries(history)]  // single partition — whole history
    };
```

If `partition()` returns `None`, the entire history is treated as one partition.

---

## 5. Building the entry list

The first internal step is converting operations into a sorted list of
**entries** — one call entry and one return entry per operation, sorted by
timestamp.

For a history with operations A (call=1, return=4) and B (call=2, return=3):

```
Sorted entries:  A.call(t=1)  B.call(t=2)  B.return(t=3)  A.return(t=4)
```

Calls are placed before returns at equal timestamps (ties broken so that
calls come first).

### Rust implementation (`src/checker.rs` — `make_entries`)

```rust
fn make_entries<I: Clone, O: Clone>(ops: &[Operation<I, O>]) -> Vec<Entry<I, O>> {
    let mut entries = Vec::with_capacity(ops.len() * 2);
    for (id, op) in ops.iter().enumerate() {
        entries.push(Entry { id, time: op.call,        value: EntryValue::Call(op.input.clone()) });
        entries.push(Entry { id, time: op.return_time, value: EntryValue::Return(op.output.clone()) });
    }
    entries.sort_by(|a, b| a.time.cmp(&b.time).then_with(|| /* calls before returns */));
    entries
}
```

Each entry carries:
- `id` — operation index (shared between call and return of the same op)
- `time` — timestamp for ordering
- `value` — either `Call(input)` or `Return(output)`

---

## 6. Building the linked list (NodeArena)

The DFS needs to efficiently **remove** (lift) and **re-insert** (unlift)
operations as it explores linearization orderings.  A doubly-linked list
supports both in O(1).

Rust's ownership model makes pointer-based doubly-linked lists awkward.  The
solution is an **index-based arena**: all nodes live in a `Vec<Node>`, and
`prev`/`next` fields are `u32` indices rather than pointers.

```
Sentinel(0) ──► Node₁(call A) ◄──► Node₂(call B) ◄──► Node₃(ret B) ◄──► Node₄(ret A)
```

A sentinel node at index 0 eliminates edge-case handling for the list head.
Call nodes store a `match_idx` pointing to their paired return node.

### Rust implementation (`src/checker.rs` — `NodeArena`)

```rust
struct Node<I, O> {
    value: Option<EntryValue<I, O>>,
    id: u32,
    match_idx: u32,   // u32::MAX if absent
    prev: u32,
    next: u32,        // u32::MAX signals end-of-list
}

struct NodeArena<I, O> {
    nodes: Vec<Node<I, O>>,
}
```

Key operations:

- **`from_entries(entries)`** — allocates all nodes in one `Vec::with_capacity`
  call, links them in order, and fills `match_idx` for each call → return pair.
- **`lift(call_ref)`** — unlinks the call node and its matched return node from
  the live list (6 index writes, no allocation).
- **`unlift(call_ref)`** — re-links both nodes back into their original
  positions (6 index writes, no allocation).
- **`head_next()`** — returns the first live node after the sentinel.
- **`next_of(r)`** / **`match_of(r)`** — accessors returning `Option<NodeRef>`.

Using `u32` indices (instead of `usize`) halves the per-node overhead on 64-bit
platforms and keeps the `Node` struct compact for better cache utilisation.
`u32::MAX` serves as a sentinel value meaning "no node" (replacing `Option`).

---

## 7. The DFS — core linearizability search

The heart of the checker is a depth-first search with backtracking.  It tries
to build a valid sequential execution by linearizing one operation at a time.

### Algorithm in plain English

1. **Start** with the model's initial state and an empty "linearized" set.
2. **Walk** the live list from the beginning.
3. At each node:
   - **Call node** — this operation's call has been seen but not yet linearized.
     Ask the model: "if we linearize this operation now (apply its input/output
     to the current state), is the transition valid?"
     - **Valid** — before committing, **probe the cache**: has this exact
       `(linearized_set ∪ {op}, model_state)` pair been explored before?
       - **Cache hit** — a previous DFS branch already exhaustively searched
         this subtree.  Skip this node (advance cursor) without any allocation.
       - **Cache miss** — record the linearization: clone the bitset, insert
         the `(bitset, state)` pair into the cache, push a frame onto the
         backtrack stack, remove the operation from the live list, and restart
         from the beginning.
     - **Invalid** — skip to the next node.
   - **Return node** — we've reached a return whose call hasn't been linearized.
     This means we must linearize that call *before* any operation whose call
     comes later (real-time ordering constraint).  We're stuck on this path
     — **backtrack**: undo the most recent linearization (pop the stack, restore
     the state, re-insert the operation into the live list) and continue
     from where we left off.
4. **End of list** — all operations have been linearized.  The history is
   linearizable.  Return `true`.
5. **Empty stack + stuck** — no valid linearization exists for this prefix.
   Return `false`.

The cache is the critical optimisation that makes the search tractable.
Without it, the DFS would revisit the same `(linearized_set, state)` from
every permutation of linearization orderings that leads to it — exponential
redundancy.  With the cache, each unique `(bitset, state)` is explored at
most once, transforming many exponential histories into polynomial searches.
See Section 8 for the cache implementation and the deferred-clone strategy.

### Why restart from the beginning?

After linearizing an operation and lifting it from the list, the set of
"current candidates" changes.  The earliest remaining call might now be a
different operation.  Restarting from the head ensures we always consider
the **minimal-call frontier** (INV-HIST-03): the operation with the
earliest call time among those not yet linearized.

### Worked example — tracing the DFS step by step

Consider a register (initially `0`) with three concurrent operations:

| Op | Kind       | Call time | Return time | Output |
|----|------------|-----------|-------------|--------|
| A  | write(1)   | 0         | 30          | —      |
| B  | write(2)   | 5         | 20          | —      |
| C  | read → 1   | 25        | 35          | 1      |

Note: A and B overlap, B finishes before A, and C starts after B finishes but
before A finishes.  The only valid linearization must have `write(1)` as the
last write before the read, so the final state when C executes must be `1`.

**Sorted entry list (linked list)**:

```
HEAD → callA(0) → callB(5) → retB(20) → callC(25) → retA(30) → retC(35)
```

Each call node stores a `match_idx` to its return: A→retA, B→retB, C→retC.

---

**Iteration 1** — `state=0`, `linearized={}`, cursor → `callA`

- `callA` is a call node (match → `retA`).  Try `model.step(0, write(1), _)` → `Some(1)`. Valid!
- Probe cache: `({A}, state=1)` — cache is empty, so **miss**.
- Commit: push `CallFrame{A, old_state=0}`, set bit A, lift A's call+return.
- Cache: `{({A}, 1)}`.

```
Live list:  HEAD → callB → retB → callC → retC
```

---

**Iteration 2** — `state=1`, `linearized={A}`, cursor → `callB` (restart from head)

- `callB` is a call node (match → `retB`).  Try `model.step(1, write(2), _)` → `Some(2)`. Valid!
- Probe cache: `({A,B}, state=2)` — **miss**.
- Commit: push `CallFrame{B, old_state=1}`, set bit B, lift B's call+return.
- Cache: `{({A}, 1), ({A,B}, 2)}`.

```
Live list:  HEAD → callC → retC
```

---

**Iteration 3** — `state=2`, `linearized={A,B}`, cursor → `callC`

- `callC` is a call node (match → `retC`).  Try `model.step(2, read, 1)` → read expects
  output == state, but `1 ≠ 2`.  **Invalid!** — model rejects.
- Advance cursor → `retC`.

---

**Iteration 4** — cursor → `retC`

- `retC` is a return node with no `match_idx` (its call hasn't been linearized yet
  relative to what the DFS expects here — actually C's call *is* in the list but
  we already passed it).  We're stuck → **backtrack**.
- Pop `CallFrame{B, old_state=1}`.  Restore: `state=1`, clear bit B, unlift B.

```
Live list:  HEAD → callB → retB → callC → retC
```

- Advance cursor past `callB` → `retB`.

---

**Iteration 5** — `state=1`, `linearized={A}`, cursor → `retB`

- `retB` is a return node — stuck again → **backtrack**.
- Pop `CallFrame{A, old_state=0}`.  Restore: `state=0`, clear bit A, unlift A.

```
Live list:  HEAD → callA → callB → retB → callC → retA → retC
```

- Advance cursor past `callA` → `callB`.

---

**Iteration 6** — `state=0`, `linearized={}`, cursor → `callB`

- `callB` is a call node.  Try `model.step(0, write(2), _)` → `Some(2)`. Valid!
- Probe cache: `({B}, state=2)` — **miss**.
- Commit: push `CallFrame{B, old_state=0}`, set bit B, lift B's call+return.
- Cache: `{({A}, 1), ({A,B}, 2), ({B}, 2)}`.

```
Live list:  HEAD → callA → callC → retA → retC
```

---

**Iteration 7** — `state=2`, `linearized={B}`, cursor → `callA` (restart)

- `callA` is a call node.  Try `model.step(2, write(1), _)` → `Some(1)`. Valid!
- Probe cache: `({A,B}, state=1)` — the cache has `({A,B}, 2)` but not
  `({A,B}, 1)`.  **Miss**.
- Commit: push `CallFrame{A, old_state=2}`, set bit A, lift A's call+return.
- Cache adds `({A,B}, 1)`.

```
Live list:  HEAD → callC → retC
```

---

**Iteration 8** — `state=1`, `linearized={A,B}`, cursor → `callC`

- `callC` is a call node.  Try `model.step(1, read, 1)` → `1 == 1`. **Valid!**
- Probe cache: `({A,B,C}, state=1)` — **miss**.
- Commit: set bit C, lift C.

```
Live list:  HEAD  (empty)
```

---

**Iteration 9** — cursor → `None` (end of list)

- All operations linearized.  Return **`true`** — the history is linearizable.
- Linearization order found: **B → A → C** (write(2), write(1), read→1). ✓

---

**What this example exercised:**

| Algorithm feature          | Where it appeared |
|----------------------------|-------------------|
| Successful linearization   | Iterations 1, 2, 6, 7, 8 |
| Model rejection (invalid)  | Iteration 3 (read→1 with state=2) |
| Return-node backtrack      | Iterations 4 and 5 |
| Cache miss (new branch)    | Iterations 1, 2, 6, 7, 8 |
| Restart from head          | After every successful lift |
| DFS completion (`None`)    | Iteration 9 |

**Cache hit example (bonus):** If we extended this history with a fourth
operation D that, after some backtracking, reached `linearized={A,B}` with
`state=2` again, the cache probe would find the entry inserted in Iteration 2.
The DFS would skip the entire subtree — no clone, no allocation, just an
`eq_with_bit` comparison returning `true`.

### Rust implementation (`src/checker.rs` — `check_single`)

```rust
fn check_single<M: Model>(model: &M, entries: Vec<Entry<...>>, kill: &AtomicBool) -> bool {
    let mut arena = NodeArena::from_entries(entries);
    let mut linearized = Bitset::new(n_ops);      // which ops are linearized
    let mut cache = FxHashMap::default();           // DFS pruning cache
    let mut calls: Vec<CallFrame<M::State>> = Vec::new();  // backtrack stack
    let mut state = model.init();
    let mut cursor = arena.head_next();

    loop {
        if kill.load(Ordering::Relaxed) { return false; }  // timeout check

        match cursor {
            None => return true,  // all linearized — success!

            Some(idx) => match arena.match_of(idx) {
                Some(ret_ref) => {
                    // Call node — try to linearize
                    let (input, output) = /* extract from call + return nodes */;

                    if let Some(next_state) = model.step(&state, input, output) {
                        let h = linearized.hash_with_bit(op_id);
                        if !cache_contains_with_bit(&cache, h, &linearized, op_id, &next_state) {
                            // Cache miss — commit this linearization
                            let mut new_linearized = linearized.clone();
                            new_linearized.set(op_id);
                            let old_state = std::mem::replace(&mut state, next_state);
                            cache.entry(h).or_default().push(CacheEntry { ... });
                            calls.push(CallFrame { node_ref: idx, state: old_state });
                            linearized.set(op_id);
                            arena.lift(idx);
                            cursor = arena.head_next();  // restart from beginning
                        } else {
                            cursor = arena.next_of(idx);  // cache hit — skip
                        }
                    } else {
                        cursor = arena.next_of(idx);  // model rejected — skip
                    }
                }

                None => {
                    // Return node — must backtrack
                    if calls.is_empty() { return false; }  // no linearization exists
                    let frame = calls.pop().unwrap();
                    state = frame.state;
                    linearized.clear(op_id);
                    arena.unlift(frame.node_ref);
                    cursor = arena.next_of(frame.node_ref);  // advance past restored call
                }
            }
        }
    }
}
```

---

## 8. The DFS cache — pruning duplicate branches

Without pruning, the DFS would re-explore the same state from different
linearization orderings.  For example, if linearizing A then B leads to the
same model state as B then A, there is no point searching both subtrees.

The cache stores `(linearized_bitset, model_state)` pairs.  Before committing a
linearization, the DFS probes the cache: if this exact combination of
"which operations are done" and "what state we're in" has been seen before,
the subtree is skipped.

### Cache key design

The cache is a hash map keyed by a `u64` hash of the bitset.  The hash is
`popcnt ⊕ w₀ ⊕ w₁ ⊕ …` — an XOR fold over the bitset words, seeded with the
population count to prevent anagram collisions.  Collisions are resolved by a
`SmallVec` of `(Bitset, State)` pairs at each bucket (almost always length 1).

### Deferred clone optimisation — `cache_contains_with_bit`

The DFS cache is probed on every successful `model.step()` call.  As the
search goes deeper, the proportion of cache hits rises sharply: most
`(bitset, state)` combinations have already been explored via a different
linearization ordering.  Minimising the cost of cache *hits* is therefore
critical to overall performance.

#### The earlier strategy: eager clone with `cache_contains`

The original approach (`cache_contains`) followed a straightforward pattern:

```text
1. model.step() succeeds → next_state
2. Clone the linearized bitset             ← allocation on every step
3. Set the new bit in the clone
4. Compute hash of the cloned bitset
5. Probe cache with (cloned_bitset, state)
6. Cache HIT  → discard the clone          ← wasted allocation
7. Cache MISS → insert clone into cache, proceed
```

Every successful `model.step()` triggered a `Bitset::clone()` *before* the
cache was consulted.  On a cache hit — the common case mid-search — the
freshly allocated clone was immediately thrown away.  For histories with
hundreds of operations and deep DFS trees, this produced millions of
short-lived allocations that pressure the allocator and pollute CPU caches.

**Concrete example.**  Suppose we are checking a history with four operations
`{A, B, C, D}` and the DFS is deep in the search tree, having already
linearized `{A, B}` (bits 0 and 1 set).  The DFS finds operation C callable:

```text
linearized = {A, B}

── try C ──────────────────────────────────────────────────
1. model.step(state, C) succeeds → next_state_C
2. Clone {A, B}        → alloc SmallVec → cloned = {A, B}
3. Set bit 2 in cloned → cloned = {A, B, C}
4. Hash cloned         → h₁
5. Probe cache (h₁, {A,B,C}, next_state_C)
6. Cache HIT — already reached via order B → A → C
   → clone DISCARDED                        ← wasted alloc

── try D ──────────────────────────────────────────────────
1. model.step(state, D) succeeds → next_state_D
2. Clone {A, B}        → alloc SmallVec → cloned = {A, B}
3. Set bit 3 in cloned → cloned = {A, B, D}
4. Hash cloned         → h₂
5. Probe cache (h₂, {A,B,D}, next_state_D)
6. Cache HIT — already explored via B → A → D
   → clone DISCARDED                        ← wasted alloc
```

Both probes were cache hits, yet each paid for a full `Bitset::clone()` — a
`SmallVec` heap copy plus bookkeeping — only to immediately discard the
result.  At depth, where the hit rate commonly exceeds 80 %, four out of
every five clones are thrown away unused.  Over 10⁴–10⁵ DFS branches this
amounts to tens of thousands of pointless allocations that:

- **pressure the allocator** — malloc / free churn on every step,
- **pollute CPU data caches** — freshly allocated memory evicts hot lines,
- **do no useful work** — the clone is never stored or read again.

#### The current strategy: virtual probe with `cache_contains_with_bit`

The key insight is that a cache probe only needs to *read* the bitset with an
extra bit logically set — it does not need a physical clone.  Two helper
methods on `Bitset` make this possible:

- **`hash_with_bit(pos)`** — computes the hash the bitset *would* have if bit
  `pos` were set, using arithmetic on a single word.  No mutation, no clone.
- **`eq_with_bit(pos, &other)`** — checks equality as-if bit `pos` were set,
  adjusting one word on the fly during the comparison loop.  No mutation, no
  clone.

The new flow:

```text
1. model.step() succeeds → next_state
2. hash_with_bit(op_id)                   ← O(1) (cached hash), no allocation
3. cache_contains_with_bit(…)             ← virtual equality, no allocation
4. Cache HIT  → skip (zero allocations)   ← the fast path stays fast
5. Cache MISS → clone bitset, set bit, insert into cache, proceed
```

The `linearized.clone()` is deferred to the cache-miss branch only, so the
hot path (cache hit) does zero allocation.  On a miss, the clone is
unavoidable because the cache needs to own a copy — but misses become
increasingly rare as the search progresses.

#### Impact

For a typical etcd history (~170 ops), the DFS explores on the order of 10⁴–10⁵
branches.  The cache hit rate in the deeper half of the search commonly exceeds
80%.  Under the old scheme, every one of those hits paid for a `Bitset::clone()`
(a `SmallVec` copy of ≤4 words plus bookkeeping).  Under the new scheme, hits
cost only a hash XOR and a word-by-word comparison — purely register-level
arithmetic.  The allocator pressure drop is proportional to the hit rate,
yielding measurable wall-clock improvements on large histories.

### Rust implementation (`src/bitset.rs`, `src/checker.rs`)

```rust
// Incremental hash — no clone needed, O(1) via cached hash
pub fn hash_with_bit(&self, pos: usize) -> u64 {
    let (major, minor) = Self::index(pos);
    let old_word = self.data[major];
    let new_word = old_word | (1u64 << minor);
    self.cached_hash ^ old_word ^ new_word ^ 1
}

// Virtual equality — no clone needed
pub fn eq_with_bit(&self, pos: usize, other: &Bitset) -> bool {
    for (i, (&a, &b)) in self.data.iter().zip(other.data.iter()).enumerate() {
        let a_adj = if i == major { a | (1u64 << minor) } else { a };
        if a_adj != b { return false; }
    }
    true
}

// Cache probe in the DFS — clone only on miss
fn cache_contains_with_bit<S: PartialEq>(
    cache: &FxHashMap<u64, SmallVec<[CacheEntry<S>; 2]>>,
    hash: u64, bitset: &Bitset, bit_pos: usize, state: &S,
) -> bool { /* check hash bucket, then eq_with_bit + state equality */ }
```

---

## 9. The bitset — tracking linearized operations

The "linearized set" tracks which operations have been linearized so far.  It
is represented as a compact bitset: bit `i` is set if operation `i` has been
linearized.

### Rust implementation (`src/bitset.rs`)

```rust
pub struct Bitset {
    data: SmallVec<[u64; 4]>,
    cached_hash: u64,
}
```

- Inline capacity of 4 words covers up to 256 operations without heap
  allocation.  Typical histories fit entirely on the stack.
- `set(pos)` / `clear(pos)` — single bit operation, incrementally updates the
  cached hash.
- `hash()` — returns the cached hash in O(1).  The hash value is equivalent to
  an XOR fold over all words seeded with `popcnt`, but is maintained
  incrementally by `set()` and `clear()` rather than recomputed.
- `popcnt()` — uses `u64::count_ones()`, which compiles to a single hardware
  `POPCNT` instruction.

---

## 10. Backtracking

When the DFS reaches a return node (meaning we've hit a point where an
operation *must* have completed but its call hasn't been linearized yet), it
backtracks:

1. Pop the most recent `CallFrame` from the stack.
2. Restore the model state to what it was before that linearization.
3. Clear the operation's bit in the linearized bitset.
4. Re-insert (unlift) the operation's call and return nodes into the live list.
5. Advance the cursor past the restored call node (to try the next candidate
   at that depth).

If the stack is empty and we're stuck, the history is **not linearizable**.

This is standard DFS backtracking, but the combination of the arena-based
linked list (`unlift` is O(1)) and the in-place bitset (`clear` is O(1))
makes each backtrack step very cheap.

---

## 11. Parallel checking and timeout

### Parallel dispatch (`check_parallel`)

After partitioning, the checker runs each partition's DFS independently.
Three modes are used depending on workload size:

| Condition              | Strategy            | Rationale                              |
|------------------------|---------------------|----------------------------------------|
| Single partition       | Direct call         | Zero dispatch overhead                 |
| < 2000 total entries   | Sequential loop     | Avoids rayon task-submission cost (~3–5 µs/partition) |
| ≥ 2000 total entries   | `rayon::par_iter`   | True parallelism on multi-core         |

A shared `Arc<AtomicBool>` **kill flag** connects all partitions: the first
partition to prove non-linearizability sets the flag, and siblings abort their
DFS within microseconds (the flag is checked at the top of every DFS iteration).

A `definitive_illegal` flag is also tracked: it distinguishes between a
partition that completed its full DFS (definitive proof) versus one killed
mid-search by a timeout.

### Timeout

If a `Duration` is provided, a background timer thread sleeps for that
duration and then sets the kill flag.  The timer uses `Condvar::wait_timeout`
so it can be cancelled immediately when the check finishes (avoiding thread
accumulation in test suites).

```
Outcome priority:
  1. Any partition proved non-linearizability definitively → Illegal
  2. Timer fired, no definitive answer                     → Unknown
  3. DFS completed, all partitions OK                      → Ok
```

### Rust implementation (`src/checker.rs`)

```rust
fn check_parallel<M>(model: &M, partitions: Vec<Vec<Entry<...>>>,
                     kill: Arc<AtomicBool>, definitive_illegal: &AtomicBool) -> bool {
    // Single partition → check_single directly
    // Small total → sequential loop
    // Large total → rayon par_iter with kill flag
}

fn spawn_timer(kill: &Arc<AtomicBool>, duration: Duration) -> TimerHandle {
    // Background thread: sleep → set timed_out + kill
    // Cancellable via Condvar
}

fn to_check_result(ok: bool, timed_out: &AtomicBool,
                   definitive_illegal: &AtomicBool) -> CheckResult {
    // Illegal > Unknown > Ok priority
}
```

---

## 12. Nondeterministic models

Some specifications are inherently nondeterministic: a single (state, input,
output) triple may lead to multiple valid successor states.

The `NondeterministicModel` trait handles this: `step` returns a `Vec<State>`
(all valid successors) instead of `Option<State>`.  An empty vec means the
transition is rejected.

The `PowerSetModel` adapter wraps a `NondeterministicModel` into a regular
`Model` via the **power-set construction**: the adapted state is the *set* of
all concrete states the system could be in.  Each `step` fans out over every
state in the current set, collects all successors, deduplicates them, and
returns `Some(set)` if non-empty or `None` if empty.

### Deduplication strategies

The power-set construction must deduplicate successor states after each step.
Two adapters are provided, trading off trait bounds against performance:

| Adapter | State bounds | Dedup cost | Use when |
|---------|-------------|------------|----------|
| `PowerSetModel` | `PartialEq` | O(n²) linear scan | Default; works with any state type |
| `HashedPowerSetModel` | `Eq + Hash` | O(n) via `HashSet` | State already implements `Eq + Hash` and branching factor is large |

`PowerSetModel` requires only `PartialEq` on the state type — the same bound
required by `NondeterministicModel` itself — so it works out of the box with
any conforming model.  Its O(n²) dedup via `Vec::contains` is adequate for
the small state sets typical of nondeterministic specifications.

`HashedPowerSetModel` is an opt-in fast path for models whose state type
already implements `Eq + Hash`.  It uses a `HashSet` for O(n) average-case
deduplication, which matters when the branching factor is large enough for the
quadratic scan to become a bottleneck.

Both adapters delegate `partition` and `partition_events` to the inner model
unchanged.

### Rust implementation (`src/model.rs`)

```rust
pub struct PowerSetModel<M>(pub M);

impl<M: NondeterministicModel> Model for PowerSetModel<M>
where M::State: Clone + PartialEq {
    type State = Vec<M::State>;  // the power-state

    fn init(&self) -> Self::State { deduplicate(self.0.init()) }

    fn step(&self, state: &Self::State, input: &Self::Input,
            output: &Self::Output) -> Option<Self::State> {
        let next: Vec<_> = state.iter()
            .flat_map(|s| self.0.step(s, input, output))
            .collect();
        let deduped = deduplicate(next);
        if deduped.is_empty() { None } else { Some(deduped) }
    }
}

pub struct HashedPowerSetModel<M>(pub M);

impl<M: NondeterministicModel> Model for HashedPowerSetModel<M>
where M::State: Clone + Eq + Hash {
    type State = Vec<M::State>;

    fn init(&self) -> Self::State { deduplicate_hashed(self.0.init()) }

    fn step(&self, state: &Self::State, input: &Self::Input,
            output: &Self::Output) -> Option<Self::State> {
        let next: Vec<_> = state.iter()
            .flat_map(|s| self.0.step(s, input, output))
            .collect();
        let deduped = deduplicate_hashed(next);
        if deduped.is_empty() { None } else { Some(deduped) }
    }
}
```

`deduplicate` uses `Vec::contains` (O(n²)); `deduplicate_hashed` uses a
`HashSet` (O(n) average).  Both preserve first-occurrence order.

---

## 13. Putting it all together

The full pipeline for `check_operations(model, history, timeout)`:

```
1.  Validate history                          INV-HIST-01
           │
2.  Partition (if model supports it)          INV-LIN-03
           │
3.  For each partition:
    ├── make_entries() → sorted call/return pairs
    ├── NodeArena::from_entries() → doubly-linked list
    └── check_single() → DFS with backtracking + cache
           │
4.  check_parallel() dispatches partitions    (rayon / sequential)
           │
5.  Combine results                           Illegal > Unknown > Ok
           │
6.  Return CheckResult
```

For an event-based history (`check_events`), steps 1–2 use `renumber` +
`convert_entries` instead of `make_entries`, but the rest is identical.

---

## 14. Complexity

| Aspect           | Cost                                        |
|------------------|---------------------------------------------|
| Time (worst)     | Exponential in number of concurrent operations — NP-complete |
| Time (typical)   | Near-linear for "almost linearizable" histories thanks to caching |
| Space            | O(n) for the arena + O(2ⁿ) worst case for the cache |
| Backtrack step   | O(1): pop frame, clear bit, unlift (6 index writes) |
| Cache probe      | O(1) amortised: hash lookup + bitset equality |
| Lift / unlift    | O(1): 6 u32 writes each                     |

The cache is the key to practical performance: it prevents re-exploring
subtrees that lead to already-seen `(bitset, state)` combinations.  For
well-behaved models (small state space, few concurrent operations), the cache
keeps the search tractable even though the worst case is exponential.
