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
     - **Valid** — record the linearization (push onto the stack, mark the
       operation as linearized, remove it from the live list), and restart from
       the beginning of the live list.
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

### Why restart from the beginning?

After linearizing an operation and lifting it from the list, the set of
"current candidates" changes.  The earliest remaining call might now be a
different operation.  Restarting from the head ensures we always consider
the **minimal-call frontier** (INV-HIST-03): the operation with the
earliest call time among those not yet linearized.

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

### Deferred clone optimisation

Before probing the cache, the checker calls `hash_with_bit(op_id)` which
computes the hash the bitset *would* have if the bit were set — without
cloning or mutating the bitset.  Similarly, `eq_with_bit(op_id, &other)`
checks equality as-if the bit were set, adjusting one word on the fly.
The actual `linearized.clone()` is deferred to the cache-miss branch,
so cache hits (the common case mid-search) avoid allocation entirely.

### Rust implementation (`src/bitset.rs`, `src/checker.rs`)

```rust
// Incremental hash — no clone needed
pub fn hash_with_bit(&self, pos: usize) -> u64 {
    let (major, minor) = Self::index(pos);
    let old_word = self.0[major];
    let new_word = old_word | (1u64 << minor);
    self.hash() ^ old_word ^ new_word ^ 1
}

// Virtual equality — no clone needed
pub fn eq_with_bit(&self, pos: usize, other: &Bitset) -> bool {
    for (i, (&a, &b)) in self.0.iter().zip(other.0.iter()).enumerate() {
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
pub struct Bitset(SmallVec<[u64; 4]>);
```

- Inline capacity of 4 words covers up to 256 operations without heap
  allocation.  Typical histories fit entirely on the stack.
- `set(pos)` / `clear(pos)` — single bit operation, no hashing.
- `hash()` — XOR fold over all words, seeded with `popcnt`.
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

### Rust implementation (`src/model.rs`)

```rust
pub struct PowerSetModel<M>(pub M);

impl<M: NondeterministicModel> Model for PowerSetModel<M> {
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
```

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
