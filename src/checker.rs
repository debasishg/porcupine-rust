// Core linearizability checking logic.
//
// Algorithm (mirrors porcupine/checker.go):
//  1. Assert INV-HIST-01 (well-formed) on entry.
//  2. Optionally split into independent partitions (INV-LIN-03).
//  3. For each partition, flatten operations into a sorted Vec<Entry>
//     (one Call + one Return per operation).
//  4. Build an index-based doubly-linked list (NodeArena) from the entries.
//  5. Run DFS with backtracking (check_single):
//     - Walk the live list; call nodes have a `match_idx` pointing at their return.
//     - For each call node: attempt model.step; on success push to `calls` stack,
//       cache (bitset, state), and lift the call+return pair from the list.
//     - For each return node (no match, meaning the call hasn't been linearized yet
//       and it's blocking progress): backtrack by popping the calls stack.
//     - Cache prunes duplicate (bitset, state) branches (INV-LIN-04).
//  6. Return Ok / Illegal.

use rayon::prelude::*;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::bitset::Bitset;
use crate::invariants::{
    assert_partition_covers_ops, assert_partition_events_paired, assert_well_formed,
    assert_well_formed_events,
};
use crate::model::Model;
use crate::types::{CheckResult, Event, EventKind, Operation};

// ---------------------------------------------------------------------------
// Internal entry representation
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum EntryValue<I, O> {
    Call(I),
    Return(O),
}

impl<I, O> EntryValue<I, O> {
    #[inline]
    fn is_call(&self) -> bool {
        matches!(self, Self::Call(_))
    }

    #[inline]
    fn is_return(&self) -> bool {
        matches!(self, Self::Return(_))
    }
}

#[derive(Clone)]
struct Entry<I, O> {
    id: usize, // operation id (0-indexed); call and return share the same id
    time: u64, // u64 to avoid silent overflow when timestamps are near u64::MAX
    value: EntryValue<I, O>,
}

/// Flatten a slice of `Operation`s into a sorted Vec of `Entry` pairs.
/// Calls precede returns at equal timestamps (mirrors Go `byTime` sort).
fn make_entries<I: Clone, O: Clone>(ops: &[Operation<I, O>]) -> Vec<Entry<I, O>> {
    let mut entries = Vec::with_capacity(ops.len() * 2);
    for (id, op) in ops.iter().enumerate() {
        entries.push(Entry {
            id,
            time: op.call,
            value: EntryValue::Call(op.input.clone()),
        });
        entries.push(Entry {
            id,
            time: op.return_time,
            value: EntryValue::Return(op.output.clone()),
        });
    }
    entries.sort_by(|a, b| {
        a.time.cmp(&b.time).then_with(|| {
            // calls before returns at equal timestamps
            match (&a.value, &b.value) {
                (EntryValue::Call(_), EntryValue::Return(_)) => std::cmp::Ordering::Less,
                (EntryValue::Return(_), EntryValue::Call(_)) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            }
        })
    });
    entries
}

/// Build entries for a partition directly from the full history and an index
/// list, assigning dense 0-based ids within the partition.  Avoids cloning
/// entire sub-histories into an intermediate `Vec<Operation>`.
fn make_entries_from_indices<I: Clone, O: Clone>(
    history: &[Operation<I, O>],
    indices: &[usize],
) -> Vec<Entry<I, O>> {
    let mut entries = Vec::with_capacity(indices.len() * 2);
    for (local_id, &global_idx) in indices.iter().enumerate() {
        let op = &history[global_idx];
        entries.push(Entry {
            id: local_id,
            time: op.call,
            value: EntryValue::Call(op.input.clone()),
        });
        entries.push(Entry {
            id: local_id,
            time: op.return_time,
            value: EntryValue::Return(op.output.clone()),
        });
    }
    entries.sort_by(|a, b| {
        a.time.cmp(&b.time).then_with(|| {
            match (&a.value, &b.value) {
                (EntryValue::Call(_), EntryValue::Return(_)) => std::cmp::Ordering::Less,
                (EntryValue::Return(_), EntryValue::Call(_)) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            }
        })
    });
    entries
}

/// Renumber `Event` IDs to be contiguous starting at 0.
fn renumber<I: Clone, O: Clone>(events: &[Event<I, O>]) -> Vec<Event<I, O>> {
    let mut out = Vec::with_capacity(events.len());
    let mut map: FxHashMap<u64, u64> = FxHashMap::default();
    let mut next_id = 0u64;
    for ev in events {
        let new_id = *map.entry(ev.id).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
        out.push(Event {
            id: new_id,
            ..ev.clone()
        });
    }
    out
}

/// Convert a renumbered slice of `Event`s into `Entry`s (index as time).
fn convert_entries<I: Clone, O: Clone>(events: &[Event<I, O>]) -> Vec<Entry<I, O>> {
    events
        .iter()
        .enumerate()
        .map(|(i, ev)| {
            let value = match ev.kind {
                EventKind::Call => {
                    EntryValue::Call(ev.input.clone().expect("Call event must have input"))
                }
                EventKind::Return => {
                    EntryValue::Return(ev.output.clone().expect("Return event must have output"))
                }
            };
            Entry {
                id: ev.id as usize,
                time: i as u64,
                value,
            }
        })
        .collect()
}

/// Build entries for a partition directly from the full event history and an
/// index list, renumbering ids and converting in a single pass.  Avoids
/// cloning entire sub-event-lists into intermediate `Vec<Event>` allocations.
fn convert_entries_from_indices<I: Clone, O: Clone>(
    history: &[Event<I, O>],
    indices: &[usize],
) -> Vec<Entry<I, O>> {
    let mut id_map: FxHashMap<u64, u64> = FxHashMap::default();
    let mut next_id = 0u64;
    let mut entries = Vec::with_capacity(indices.len());
    for (local_time, &global_idx) in indices.iter().enumerate() {
        let ev = &history[global_idx];
        let new_id = *id_map.entry(ev.id).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
        let value = match ev.kind {
            EventKind::Call => {
                EntryValue::Call(ev.input.clone().expect("Call event must have input"))
            }
            EventKind::Return => {
                EntryValue::Return(ev.output.clone().expect("Return event must have output"))
            }
        };
        entries.push(Entry {
            id: new_id as usize,
            time: local_time as u64,
            value,
        });
    }
    entries
}

// ---------------------------------------------------------------------------
// Index-based doubly-linked list (NodeArena)
// ---------------------------------------------------------------------------

// Sentinel HEAD is always at index 0.
// All real nodes occupy indices 1 ..= 2n.
//
// `value` is `None` only for the sentinel; always `Some` for real nodes.
struct Node<I, O> {
    value: Option<EntryValue<I, O>>,
    id: u32,
    match_idx: u32,   // NodeRef::NONE if absent
    prev: u32,
    next: u32,        // NodeRef::NONE if absent
}

struct NodeArena<I, O> {
    nodes: Vec<Node<I, O>>,
}

/// Typed index into a [`NodeArena`]. The sentinel HEAD is always `NodeRef(0)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NodeRef(u32);

impl NodeRef {
    const NONE: u32 = u32::MAX;

    #[inline]
    fn get(self) -> usize {
        self.0 as usize
    }

    #[inline]
    fn raw(self) -> u32 {
        self.0
    }
}

impl<I, O> NodeArena<I, O> {
    /// Build the arena from a sorted entry list.
    fn from_entries(entries: Vec<Entry<I, O>>) -> Self {
        let n = entries.len();
        let mut arena_nodes: Vec<Node<I, O>> = Vec::with_capacity(n + 1);

        // Sentinel at index 0 — value is None (never accessed in DFS).
        arena_nodes.push(Node {
            value: None,
            match_idx: NodeRef::NONE,
            id: NodeRef::NONE,
            prev: 0,
            next: NodeRef::NONE,
        });

        // Op ids are dense 0..n_ops, so a Vec lookup is faster than a HashMap.
        let n_ops = n / 2;
        let mut return_idx: Vec<u32> = vec![NodeRef::NONE; n_ops];

        // Allocate a slot for each entry.
        for (i, entry) in entries.into_iter().enumerate() {
            let node_ref = NodeRef((i + 1) as u32); // 1-indexed
            if entry.value.is_return() {
                return_idx[entry.id] = node_ref.raw();
            }
            arena_nodes.push(Node {
                value: Some(entry.value),
                match_idx: NodeRef::NONE, // filled in next pass
                id: entry.id as u32,
                prev: 0,
                next: NodeRef::NONE,
            });
        }

        // Fill match_idx for call nodes.
        for node in arena_nodes.iter_mut().skip(1) {
            if node.value.as_ref().is_some_and(|v| v.is_call()) {
                let op_id = node.id as usize;
                let ret_ref = return_idx[op_id];
                if ret_ref != NodeRef::NONE {
                    node.match_idx = ret_ref;
                }
            }
        }

        // Link nodes in order: sentinel → 1 → 2 → … → n
        for (i, node) in arena_nodes.iter_mut().enumerate().skip(1) {
            node.prev = (i - 1) as u32;
            if i < n {
                node.next = (i + 1) as u32;
            }
        }
        arena_nodes[0].next = if n > 0 { 1 } else { NodeRef::NONE };

        NodeArena { nodes: arena_nodes }
    }

    /// Index of the first live node after sentinel HEAD.
    #[inline]
    fn head_next(&self) -> Option<NodeRef> {
        let n = self.nodes[0].next;
        if n == NodeRef::NONE { None } else { Some(NodeRef(n)) }
    }

    /// Next node after `r`, or `None` if at end.
    #[inline]
    fn next_of(&self, r: NodeRef) -> Option<NodeRef> {
        let n = self.nodes[r.get()].next;
        if n == NodeRef::NONE { None } else { Some(NodeRef(n)) }
    }

    /// Match index for a call node, or `None` for return/sentinel.
    #[inline]
    fn match_of(&self, r: NodeRef) -> Option<NodeRef> {
        let m = self.nodes[r.get()].match_idx;
        if m == NodeRef::NONE { None } else { Some(NodeRef(m)) }
    }

    /// Remove `call_ref` and its matched return node from the live list.
    #[inline]
    fn lift(&mut self, call_ref: NodeRef) {
        let match_raw = self.nodes[call_ref.get()].match_idx;
        debug_assert_ne!(match_raw, NodeRef::NONE);
        let match_idx = match_raw as usize;

        // Unlink call node.
        let call_prev = self.nodes[call_ref.get()].prev as usize;
        let call_next = self.nodes[call_ref.get()].next;
        self.nodes[call_prev].next = call_next;
        if call_next != NodeRef::NONE {
            self.nodes[call_next as usize].prev = call_prev as u32;
        }

        // Unlink return node.
        let ret_prev = self.nodes[match_idx].prev as usize;
        let ret_next = self.nodes[match_idx].next;
        self.nodes[ret_prev].next = ret_next;
        if ret_next != NodeRef::NONE {
            self.nodes[ret_next as usize].prev = ret_prev as u32;
        }
    }

    /// Re-insert `call_ref` and its matched return node back into the live list.
    #[inline]
    fn unlift(&mut self, call_ref: NodeRef) {
        let match_raw = self.nodes[call_ref.get()].match_idx;
        debug_assert_ne!(match_raw, NodeRef::NONE);
        let match_idx = match_raw as usize;

        // Re-link return node.
        let ret_prev = self.nodes[match_idx].prev as usize;
        let ret_next = self.nodes[match_idx].next;
        self.nodes[ret_prev].next = match_raw;
        if ret_next != NodeRef::NONE {
            self.nodes[ret_next as usize].prev = match_raw;
        }

        // Re-link call node.
        let call_prev = self.nodes[call_ref.get()].prev as usize;
        let call_next = self.nodes[call_ref.get()].next;
        self.nodes[call_prev].next = call_ref.raw();
        if call_next != NodeRef::NONE {
            self.nodes[call_next as usize].prev = call_ref.raw();
        }
    }
}

// ---------------------------------------------------------------------------
// DFS cache
// ---------------------------------------------------------------------------

/// DFS cache's value type: a (bitset, state) pair representing a linearization prefix and the
/// corresponding model state.
/// The Bitset models the set of operations that have been linearized so far in a 
/// particular DFS path.

/// Each concurrent operation in the history is assigned a numeric ID (op_id). 
/// Setting bit i in the Bitset means "operation i has already been placed into the 
/// linearization order." So the Bitset is essentially a compact representation of a 
/// subset of operations.
/// 
/// Why a bitset and not, say, a HashSet<usize>: The set of linearized ops only needs 
/// membership tracking and equality comparison. A bitset is far more compact 
/// (one bit per operation, inline up to 256 ops via SmallVec<[u64; 4]>) and supports 
/// O(1) set/test, plus cheap hashing and equality — critical since cache probes happen 
/// on every DFS step.
struct CacheEntry<S> {
    linearized: Bitset,
    state: S,
}

/// Probes the DFS cache as if `bit_pos` were already set in `bitset`,
/// **without cloning or mutating the bitset**.
///
/// # Why this exists (deferred-clone optimisation)
///
/// The earlier `cache_contains` required the caller to clone the bitset and
/// set the new bit *before* probing — paying for an allocation on every
/// successful `model.step()`.  On a cache hit (the common case once the
/// search is deep), that clone was immediately discarded.
///
/// `cache_contains_with_bit` sidesteps the clone entirely:
/// - `hash` is computed via `Bitset::hash_with_bit`, which XOR-adjusts a
///   single word — pure arithmetic, no allocation.
/// - Equality is checked via `Bitset::eq_with_bit`, which OR-adjusts the
///   relevant word on the fly during the comparison loop.
///
/// The result: cache hits (often >80% of probes mid-search) cost only
/// register-level arithmetic.  The `Bitset::clone()` is deferred to the
/// cache-miss branch in `check_single`, where it is actually needed to
/// store the new entry.
#[inline]
fn cache_contains_with_bit<S: PartialEq>(
    cache: &FxHashMap<u64, SmallVec<[CacheEntry<S>; 2]>>,
    hash: u64,
    bitset: &Bitset,
    bit_pos: usize,
    state: &S,
) -> bool {
    if let Some(entries) = cache.get(&hash) {
        for e in entries {
            // eq_with_bit: compares `bitset` against `e.linearized` as if
            // `bit_pos` were set in `bitset`, adjusting one word on the fly.
            // No clone, no mutation — the bitset is borrowed immutably.
            if bitset.eq_with_bit(bit_pos, &e.linearized) && &e.state == state {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// DFS call-stack frame
// ---------------------------------------------------------------------------

struct CallFrame<S> {
    node_ref: NodeRef, // typed reference to the call node that was linearized
    state: S,          // model state *before* this linearization step
}

// ---------------------------------------------------------------------------
// check_single — the core DFS
// ---------------------------------------------------------------------------

fn check_single<M: Model>(
    model: &M,
    entries: Vec<Entry<M::Input, M::Output>>,
    kill: &AtomicBool,
) -> bool {
    if entries.is_empty() {
        return true;
    }

    let n_ops = entries.len() / 2; // number of operations
    let mut arena = NodeArena::from_entries(entries);
    let mut linearized = Bitset::new(n_ops);
    //
    // DFS cache — `FxHashMap<u64, SmallVec<[CacheEntry<M::State>; 2]>>`
    //
    // This is a hash map from `u64` to `SmallVec<[CacheEntry<S>; 2]>`:
    //
    // - Key (`u64`): A precomputed hash of the `Bitset` (the set of
    //   linearized operations).  This is computed via `Bitset::hash_with_bit`
    //   — cheap arithmetic, no allocation.
    //
    // - Value (`SmallVec<[CacheEntry<S>; 2]>`): A small vector of cache
    //   entries that share the same bitset hash.  The `SmallVec` with inline
    //   capacity 2 means most hash buckets store their entries on the stack
    //   without heap allocation (hash collisions on the bitset hash are rare
    //   but possible, so it is a list).
    //
    // Each `CacheEntry` records a `(linearized: Bitset, state: S)` pair.
    // The `Bitset` models the set of operations that have been linearized so
    // far in a particular DFS path — setting bit `i` means "operation `i`
    // has already been placed into the linearization order."  The checker
    // prunes a DFS branch when both the linearized set and the model state
    // match an already-explored entry (INV-LIN-04 / cache soundness).
    //
    // Lookup flow:
    //   1. Hash the bitset → get the `u64` key.
    //   2. Look up the bucket in `FxHashMap`.
    //   3. Linear-scan the `SmallVec` comparing `(bitset, state)` pairs for
    //      exact equality.
    //
    // `FxHashMap` (from the `rustc-hash` crate) is used instead of
    // `std::HashMap` because it uses a faster, non-cryptographic hash
    // function — appropriate here since the keys are already hashes and
    // there is no adversarial-input concern.
    //
    let mut cache: FxHashMap<u64, SmallVec<[CacheEntry<M::State>; 2]>> = FxHashMap::default();
    let mut calls: Vec<CallFrame<M::State>> = Vec::new();
    let mut state = model.init();

    let mut cursor = arena.head_next();

    loop {
        if kill.load(Ordering::Relaxed) {
            return false;
        }

        match cursor {
            None => {
                // All operations linearized successfully.
                return true;
            }
            Some(idx) => {
                match arena.match_of(idx) {
                    Some(ret_ref) => {
                        // This is a call node. Candidate for linearization. Try to linearize it.
                        // INV-HIST-03: the live list is always time-sorted, and we restart
                        // from head_next() after each lift, so the first call node we visit
                        // is always the minimal one (no unlinearized op has an earlier call).
                        let op_id = arena.nodes[idx.get()].id as usize;
                        let (input, output) = match (
                            arena.nodes[idx.get()].value.as_ref().unwrap(),
                            arena.nodes[ret_ref.get()].value.as_ref().unwrap(),
                        ) {
                            (EntryValue::Call(i), EntryValue::Return(o)) => (i, o),
                            _ => unreachable!("match_idx must point to a Return node"),
                        };

                        if let Some(next_state) = model.step(&state, input, output) {
                            // Deferred-clone cache probe:
                            // hash_with_bit computes the hash the bitset *would* have
                            // if op_id were set — O(1) arithmetic, no clone.
                            let h = linearized.hash_with_bit(op_id);

                            // cache_contains_with_bit uses eq_with_bit internally so
                            // the entire probe (hash lookup + bitset comparison + state
                            // equality) runs against the *unmodified* bitset.  On a
                            // cache hit we skip immediately — zero allocations.  The
                            // earlier cache_contains required cloning the bitset before
                            // the probe, wasting an allocation on every hit.
                            if !cache_contains_with_bit(&cache, h, &linearized, op_id, &next_state) {
                                // INV-LIN-04: new (bitset, state) pair — safe to cache.
                                // Clone deferred to here: only cache misses pay for it.
                                let mut new_linearized = linearized.clone();
                                new_linearized.set(op_id);

                                // Move old state onto the backtrack stack (no clone), then
                                // replace current state with next_state and clone once for
                                // the cache entry — halves state clone count per step.
                                let old_state = std::mem::replace(&mut state, next_state);
                                cache
                                    .entry(h)
                                    .or_insert_with(SmallVec::new)
                                    .push(CacheEntry {
                                        linearized: new_linearized,
                                        state: state.clone(), // state == next_state
                                    });
                                calls.push(CallFrame {
                                    node_ref: idx,
                                    state: old_state, // moved, no clone
                                });
                                linearized.set(op_id);
                                arena.lift(idx);
                                cursor = arena.head_next();
                            } else {
                                // Already explored this (bitset, state) — skip.
                                cursor = arena.next_of(idx);
                            }
                        } else {
                            // Model rejected this linearization point — try next.
                            cursor = arena.next_of(idx);
                        }
                    }
                    None => {
                        // This is a return node with no linearized call preceding it.
                        // We're stuck — backtrack.
                        if calls.is_empty() {
                            return false;
                        }
                        let frame = calls.pop().unwrap();
                        let call_op_id = arena.nodes[frame.node_ref.get()].id as usize;
                        state = frame.state;
                        linearized.clear(call_op_id);
                        arena.unlift(frame.node_ref);
                        // Advance past the restored call node.
                        cursor = arena.next_of(frame.node_ref);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// check_parallel — run check_single per partition in parallel via rayon
//
// Mirrors Go's checkParallel: all partitions start concurrently; the first
// Illegal result sets the kill flag so siblings abort within microseconds.
// ---------------------------------------------------------------------------

/// Returns `true` if all partitions are linearizable, `false` if any failed or
/// the kill flag was set externally (e.g. by a timeout timer).
///
/// `definitive_illegal` is set to `true` when a partition completes its full DFS
/// and proves non-linearizability (as opposed to being killed mid-search by a
/// timeout).  This lets `to_check_result` give `Illegal` priority over `Unknown`,
/// matching Go's `checkParallel` semantics.
fn check_parallel<M>(
    model: &M,
    mut partitions: Vec<Vec<Entry<M::Input, M::Output>>>,
    kill: Arc<AtomicBool>,
    definitive_illegal: &AtomicBool,
) -> bool
where
    M: Model + Sync,
    M::Input: Send,
    M::Output: Send,
{
    if partitions.is_empty() {
        return true;
    }

    // Fast path: single partition avoids all rayon task-submission overhead.
    // For models without partitioning (e.g. EtcdModel), this is always taken.
    if partitions.len() == 1 {
        let ok = check_single(model, partitions.into_iter().next().unwrap(), &kill);
        if !ok {
            // Only mark definitive if the kill flag was not already set — mirrors
            // the same race-free check in the multi-partition rayon path below.
            if !kill.load(Ordering::Relaxed) {
                definitive_illegal.store(true, Ordering::Relaxed);
            }
            kill.store(true, Ordering::Relaxed);
        }
        return ok;
    }

    // Sort ascending by partition size: smaller partitions run first.
    // For bad histories this maximises the chance that a violation-containing
    // partition finishes early, broadcasting `kill` to abort the others.
    partitions.sort_unstable_by_key(|p| p.len());

    // Sequential fast path for small total work: rayon task dispatch costs
    // ~3–5 µs per partition, which dominates when each partition is tiny
    // (e.g. KV c10: 10 partitions × ~30–80 entries ≈ 700 total entries).
    // Threshold set well above c10 (~700) and below c50 (~5× larger) so that
    // small workloads run sequentially while large ones keep rayon parallelism.
    // Etcd is unaffected: it always takes the single-partition fast path above.
    const SEQUENTIAL_THRESHOLD: usize = 2000;
    let total_entries: usize = partitions.iter().map(|p| p.len()).sum();
    if total_entries < SEQUENTIAL_THRESHOLD {
        for partition in partitions {
            if kill.load(Ordering::Relaxed) {
                return false;
            }
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

    // Re-sort largest-first for rayon: the longest-pole partition starts
    // immediately, maximising thread utilisation when partition sizes are
    // unbalanced (common in KV models with skewed key distributions).
    // The ascending sort above already served the sequential path.
    partitions.sort_unstable_by_key(|p| std::cmp::Reverse(p.len()));

    let found_illegal = partitions.into_par_iter().any(|partition| {
        // If kill was set externally (timeout or sibling Illegal), abort without
        // claiming Illegal — the caller will inspect the timed_out flag.
        if kill.load(Ordering::Relaxed) {
            return false;
        }
        let ok = check_single(model, partition, &kill);
        if !ok {
            // Record whether this was a definitive finding (kill was not yet set
            // when check_single began — it completed its full search).  There is
            // a benign race: kill may be set between check_single returning and
            // the load below, but a false-negative here only means we report
            // Unknown instead of Illegal, which is the safe direction.
            if !kill.load(Ordering::Relaxed) {
                definitive_illegal.store(true, Ordering::Relaxed);
            }
            kill.store(true, Ordering::Relaxed);
        }
        !ok
    });

    !found_illegal
}

// ---------------------------------------------------------------------------
// Timeout infrastructure
// ---------------------------------------------------------------------------

/// Handle returned by [`spawn_timer`].
///
/// Holds both the read-side flag (`timed_out`, written by the timer thread and
/// read by [`to_check_result`]) and the write-side cancel signal (written by
/// the checker after `check_parallel` returns, read by the timer thread).
struct TimerHandle {
    timed_out: Arc<AtomicBool>,
    cancel: Arc<(Mutex<bool>, Condvar)>,
}

impl TimerHandle {
    /// Wake the timer thread immediately so it exits without sleeping the full
    /// duration.  Safe to call more than once (subsequent calls are no-ops).
    fn cancel(&self) {
        let (lock, cvar) = &*self.cancel;
        *lock.lock().unwrap() = true;
        cvar.notify_one();
    }
}

/// Spawns a background timer thread that sets `kill` (and `timed_out`) after
/// `duration`, unless [`TimerHandle::cancel`] is called first.
///
/// This lets the timer thread exit as soon as the check finishes, avoiding
/// thread accumulation in test suites that use short histories with long
/// timeouts.
fn spawn_timer(kill: &Arc<AtomicBool>, duration: Duration) -> TimerHandle {
    let timed_out = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new((Mutex::new(false), Condvar::new()));
    let kill_clone = Arc::clone(kill);
    let timed_out_clone = Arc::clone(&timed_out);
    let cancel_clone = Arc::clone(&cancel);
    std::thread::spawn(move || {
        let (lock, cvar) = &*cancel_clone;
        let guard = lock.lock().unwrap();
        let (cancelled, _) = cvar.wait_timeout(guard, duration).unwrap();
        if !*cancelled {
            timed_out_clone.store(true, Ordering::Relaxed);
            kill_clone.store(true, Ordering::Relaxed);
        }
    });
    TimerHandle { timed_out, cancel }
}

/// Translate `(ok, timed_out, definitive_illegal)` into a [`CheckResult`].
///
/// Priority (matches Go's `checkParallel`):
///  1. If any partition definitively proved non-linearizability → `Illegal`.
///  2. If the timer fired and no definitive answer was found   → `Unknown`.
///  3. Otherwise the DFS completed and all partitions were ok  → `Ok`.
fn to_check_result(
    ok: bool,
    timed_out: &AtomicBool,
    definitive_illegal: &AtomicBool,
) -> CheckResult {
    if !ok && definitive_illegal.load(Ordering::Relaxed) {
        CheckResult::Illegal
    } else if timed_out.load(Ordering::Relaxed) {
        CheckResult::Unknown
    } else if ok {
        CheckResult::Ok
    } else {
        CheckResult::Illegal
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Check an operation-based history for linearizability.
///
/// `timeout` bounds the search: if the DFS has not finished within the given
/// [`Duration`], the function returns [`CheckResult::Unknown`] rather than
/// blocking indefinitely.  Pass `None` for an unbounded check (equivalent to
/// `timeout = 0` in the Go original).
///
/// Partitions returned by [`Model::partition`] are checked concurrently on the
/// rayon thread pool, matching Go's goroutine-per-partition behaviour.
pub fn check_operations<M>(
    model: &M,
    history: &[Operation<M::Input, M::Output>],
    timeout: Option<Duration>,
) -> CheckResult
where
    M: Model + Sync,
    M::Input: Send,
    M::Output: Send,
{
    // INV-HIST-01
    assert_well_formed!(history);

    let partitions: Vec<Vec<Entry<M::Input, M::Output>>> =
        if let Some(parts) = model.partition(history) {
            // INV-LIN-03: no duplicates, full coverage, bounds check
            assert_partition_covers_ops!(parts, history.len());
            parts
                .iter()
                .map(|indices| make_entries_from_indices(history, indices))
                .collect()
        } else {
            vec![make_entries(history)]
        };

    let kill = Arc::new(AtomicBool::new(false));
    let timer = timeout.map(|d| spawn_timer(&kill, d));
    let definitive_illegal = AtomicBool::new(false);

    let ok = check_parallel(model, partitions, kill, &definitive_illegal);
    if let Some(t) = &timer {
        t.cancel();
    }
    let timed_out = timer.map_or_else(|| Arc::new(AtomicBool::new(false)), |t| t.timed_out);
    to_check_result(ok, &timed_out, &definitive_illegal)
}

/// Check an event-based history for linearizability.
///
/// `timeout` works identically to [`check_operations`]: `None` means unbounded.
///
/// Partitions returned by [`Model::partition_events`] are checked concurrently
/// on the rayon thread pool.
pub fn check_events<M>(
    model: &M,
    history: &[Event<M::Input, M::Output>],
    timeout: Option<Duration>,
) -> CheckResult
where
    M: Model + Sync,
    M::Input: Send,
    M::Output: Send,
{
    // INV-HIST-01 (event form): every id has exactly one Call and one Return,
    // Call has input=Some, Return has output=Some, and Call precedes its Return.
    assert_well_formed_events!(history);

    let partitions: Vec<Vec<Entry<M::Input, M::Output>>> =
        if let Some(parts) = model.partition_events(history) {
            // INV-LIN-03: no duplicates, full coverage, bounds, paired events
            assert_partition_events_paired!(parts, history);
            parts
                .iter()
                .map(|indices| convert_entries_from_indices(history, indices))
                .collect()
        } else {
            vec![convert_entries(&renumber(history))]
        };

    let kill = Arc::new(AtomicBool::new(false));
    let timer = timeout.map(|d| spawn_timer(&kill, d));
    let definitive_illegal = AtomicBool::new(false);

    let ok = check_parallel(model, partitions, kill, &definitive_illegal);
    if let Some(t) = &timer {
        t.cancel();
    }
    let timed_out = timer.map_or_else(|| Arc::new(AtomicBool::new(false)), |t| t.timed_out);
    to_check_result(ok, &timed_out, &definitive_illegal)
}

// ===========================================================================
// Unit tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::types::{CheckResult, Event, EventKind, Operation};

    // -----------------------------------------------------------------------
    // Minimal integer register model used throughout these tests.
    //
    // State:   i32   (current register value; initialised to 0)
    // Input:   RegInput { is_write, value }
    // Output:  i32   (observed register value for reads; ignored for writes)
    //
    // step:
    //   write(v) → always valid, next state = v
    //   read      → valid iff output == state; next state unchanged
    // -----------------------------------------------------------------------

    #[derive(Clone)]
    struct Reg;

    #[derive(Clone, Debug, PartialEq)]
    struct RegInput {
        is_write: bool,
        value: i32,
    }

    impl Model for Reg {
        type State = i32;
        type Input = RegInput;
        type Output = i32;

        fn init(&self) -> i32 {
            0
        }

        fn step(&self, state: &i32, input: &RegInput, output: &i32) -> Option<i32> {
            if input.is_write {
                Some(input.value)
            } else if *output == *state {
                Some(*state)
            } else {
                None
            }
        }
    }

    // -----------------------------------------------------------------------
    // Helper constructors
    // -----------------------------------------------------------------------

    fn write(v: i32) -> RegInput {
        RegInput {
            is_write: true,
            value: v,
        }
    }
    fn read() -> RegInput {
        RegInput {
            is_write: false,
            value: 0,
        }
    }

    fn op(id: u64, input: RegInput, output: i32, call: u64, ret: u64) -> Operation<RegInput, i32> {
        Operation {
            client_id: id,
            input,
            output,
            call,
            return_time: ret,
        }
    }

    fn call_ev(id: u64, input: RegInput) -> Event<RegInput, i32> {
        Event {
            client_id: id,
            kind: EventKind::Call,
            input: Some(input),
            output: None,
            id,
        }
    }
    fn ret_ev(id: u64, output: i32) -> Event<RegInput, i32> {
        Event {
            client_id: id,
            kind: EventKind::Return,
            input: None,
            output: Some(output),
            id,
        }
    }

    // -----------------------------------------------------------------------
    // make_entries
    // -----------------------------------------------------------------------

    #[test]
    fn make_entries_empty_produces_no_entries() {
        let entries = make_entries::<RegInput, i32>(&[]);
        assert!(entries.is_empty());
    }

    #[test]
    fn make_entries_single_op_produces_two_entries() {
        let entries = make_entries(&[op(0, write(1), 0, 5, 15)]);
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0].value, EntryValue::Call(_)));
        assert!(matches!(entries[1].value, EntryValue::Return(_)));
        assert_eq!(entries[0].time, 5);
        assert_eq!(entries[1].time, 15);
        assert_eq!(entries[0].id, 0);
        assert_eq!(entries[1].id, 0);
    }

    #[test]
    fn make_entries_call_before_return_at_equal_timestamps() {
        // Instantaneous op (call == return_time). INV-HIST-02 tie-breaking:
        // Call must sort before Return at equal timestamps.
        let entries = make_entries(&[op(0, write(1), 0, 10, 10)]);
        assert!(
            matches!(entries[0].value, EntryValue::Call(_)),
            "Call must precede Return when timestamps are equal"
        );
    }

    #[test]
    fn make_entries_time_sorted_across_two_ops() {
        // op A: call=5, ret=15   op B: call=0, ret=10
        // Expected order: CallB(0), CallA(5), RetB(10), RetA(15)
        let entries = make_entries(&[op(0, write(1), 0, 5, 15), op(1, write(2), 0, 0, 10)]);
        assert_eq!(entries.len(), 4);
        assert_eq!(
            [
                entries[0].time,
                entries[1].time,
                entries[2].time,
                entries[3].time
            ],
            [0, 5, 10, 15]
        );
        assert!(matches!(entries[0].value, EntryValue::Call(_)));
        assert!(matches!(entries[1].value, EntryValue::Call(_)));
        assert!(matches!(entries[2].value, EntryValue::Return(_)));
        assert!(matches!(entries[3].value, EntryValue::Return(_)));
    }

    #[test]
    fn make_entries_large_timestamps_do_not_overflow() {
        // Pre-fix, timestamps near u64::MAX were cast to i64, wrapping to
        // negative values and inverting the sort order.
        let t = u64::MAX - 10;
        let entries = make_entries(&[
            op(0, write(1), 0, t, t + 5),
            op(1, write(2), 0, t + 1, t + 6),
        ]);
        // Expected: CallA(t), CallB(t+1), RetA(t+5), RetB(t+6)
        assert_eq!(entries[0].id, 0);
        assert_eq!(entries[1].id, 1);
        assert!(entries[0].time < entries[1].time);
        assert!(entries[1].time < entries[2].time);
        assert!(entries[2].time < entries[3].time);
    }

    // -----------------------------------------------------------------------
    // renumber
    // -----------------------------------------------------------------------

    #[test]
    fn renumber_empty_produces_empty() {
        let out = renumber::<RegInput, i32>(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn renumber_contiguous_ids_are_unchanged() {
        let events = vec![
            call_ev(0, write(1)),
            ret_ev(0, 0),
            call_ev(1, read()),
            ret_ev(1, 0),
        ];
        let out = renumber(&events);
        assert_eq!([out[0].id, out[1].id, out[2].id, out[3].id], [0, 0, 1, 1]);
    }

    #[test]
    fn renumber_noncontiguous_ids_become_0_based() {
        let events = vec![
            Event {
                client_id: 0,
                kind: EventKind::Call,
                input: Some(write(5)),
                output: None,
                id: 100,
            },
            Event {
                client_id: 0,
                kind: EventKind::Return,
                input: None,
                output: Some(0),
                id: 100,
            },
            Event {
                client_id: 1,
                kind: EventKind::Call,
                input: Some(read()),
                output: None,
                id: 999,
            },
            Event {
                client_id: 1,
                kind: EventKind::Return,
                input: None,
                output: Some(5),
                id: 999,
            },
        ];
        let out = renumber(&events);
        // Call and Return for the same op share their new id.
        assert_eq!(out[0].id, out[1].id);
        assert_eq!(out[2].id, out[3].id);
        // The two ops get distinct ids in {0, 1}.
        assert_ne!(out[0].id, out[2].id);
        assert!(out[0].id < 2 && out[2].id < 2);
    }

    #[test]
    fn renumber_preserves_event_kind_and_payload() {
        let events = vec![call_ev(7, write(42)), ret_ev(7, 0)];
        let out = renumber(&events);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0].kind, EventKind::Call));
        assert!(matches!(out[1].kind, EventKind::Return));
        assert_eq!(out[0].id, out[1].id);
    }

    // -----------------------------------------------------------------------
    // convert_entries
    // -----------------------------------------------------------------------

    #[test]
    fn convert_entries_uses_slice_index_as_time() {
        let events = vec![call_ev(0, write(1)), ret_ev(0, 0)];
        let entries = convert_entries(&events);
        assert_eq!(entries[0].time, 0);
        assert_eq!(entries[1].time, 1);
    }

    #[test]
    fn convert_entries_maps_kinds_and_ids_correctly() {
        let events = vec![call_ev(0, write(1)), ret_ev(0, 0)];
        let entries = convert_entries(&events);
        assert!(matches!(entries[0].value, EntryValue::Call(_)));
        assert!(matches!(entries[1].value, EntryValue::Return(_)));
        assert_eq!(entries[0].id, 0);
        assert_eq!(entries[1].id, 0);
    }

    // -----------------------------------------------------------------------
    // NodeArena — lift / unlift symmetry
    // -----------------------------------------------------------------------

    #[test]
    fn arena_lift_and_unlift_restores_two_op_list() {
        // op A: [0,15]  op B: [5,10]  (A wraps B)
        // Sorted entries: 1=callA, 2=callB, 3=retB, 4=retA  match: 1↔4, 2↔3
        let entries = make_entries(&[op(0, write(1), 0, 0, 15), op(1, write(2), 0, 5, 10)]);
        let mut arena = NodeArena::from_entries(entries);

        arena.lift(NodeRef(1)); // remove nodes 1 and 4 → list: 0 → 2 → 3
        assert_eq!(arena.head_next(), Some(NodeRef(2)));
        assert_eq!(arena.nodes[2].next, 3);
        assert_eq!(arena.nodes[3].next, NodeRef::NONE);

        arena.unlift(NodeRef(1)); // full list restored: 0 → 1 → 2 → 3 → 4
        assert_eq!(arena.head_next(), Some(NodeRef(1)));
        assert_eq!(arena.nodes[1].next, 2);
        assert_eq!(arena.nodes[2].next, 3);
        assert_eq!(arena.nodes[3].next, 4);
        assert_eq!(arena.nodes[4].next, NodeRef::NONE);
    }

    #[test]
    fn arena_nested_lift_unlift_restores_three_op_list() {
        // Three ops: A[0,30], B[5,20], C[25,35].
        // Sorted: callA(1), callB(2), retB(3), callC(4), retA(5), retC(6)
        //         match: 1↔5, 2↔3, 4↔6
        let entries = make_entries(&[
            op(0, write(1), 0, 0, 30),
            op(1, write(2), 0, 5, 20),
            op(2, read(), 1, 25, 35),
        ]);
        let mut arena = NodeArena::from_entries(entries);

        arena.lift(NodeRef(1)); // remove 1,5 → list: 0→2→3→4→6
        arena.lift(NodeRef(2)); // remove 2,3 → list: 0→4→6
        assert_eq!(arena.head_next(), Some(NodeRef(4)));
        assert_eq!(arena.nodes[4].next, 6);

        arena.unlift(NodeRef(2)); // restore 2,3 → list: 0→2→3→4→6
        assert_eq!(arena.head_next(), Some(NodeRef(2)));
        assert_eq!(arena.nodes[2].next, 3);
        assert_eq!(arena.nodes[3].next, 4);

        arena.unlift(NodeRef(1)); // restore 1,5 → full list: 0→1→2→3→4→5→6
        assert_eq!(arena.head_next(), Some(NodeRef(1)));
        assert_eq!(arena.nodes[1].next, 2);
        assert_eq!(arena.nodes[4].next, 5);
        assert_eq!(arena.nodes[5].next, 6);
        assert_eq!(arena.nodes[6].next, NodeRef::NONE);
    }

    // -----------------------------------------------------------------------
    // check_operations
    // -----------------------------------------------------------------------

    #[test]
    fn ops_empty_history_is_ok() {
        assert_eq!(check_operations(&Reg, &[], None), CheckResult::Ok);
    }

    #[test]
    fn ops_single_write_is_ok() {
        assert_eq!(
            check_operations(&Reg, &[op(0, write(42), 0, 0, 10)], None),
            CheckResult::Ok
        );
    }

    #[test]
    fn ops_single_read_returning_init_value_is_ok() {
        assert_eq!(
            check_operations(&Reg, &[op(0, read(), 0, 0, 10)], None),
            CheckResult::Ok
        );
    }

    #[test]
    fn ops_single_read_returning_wrong_value_is_illegal() {
        // Init state = 0; no write has occurred; read returning 42 is illegal.
        assert_eq!(
            check_operations(&Reg, &[op(0, read(), 42, 0, 10)], None),
            CheckResult::Illegal
        );
    }

    #[test]
    fn ops_sequential_write_then_correct_read_is_ok() {
        let history = [op(0, write(5), 0, 0, 10), op(1, read(), 5, 11, 20)];
        assert_eq!(check_operations(&Reg, &history, None), CheckResult::Ok);
    }

    #[test]
    fn ops_sequential_read_after_write_returning_stale_value_is_illegal() {
        // write(5) finishes at t=10; read starts at t=11 (no overlap) but returns 0.
        let history = [op(0, write(5), 0, 0, 10), op(1, read(), 0, 11, 20)];
        assert_eq!(check_operations(&Reg, &history, None), CheckResult::Illegal);
    }

    #[test]
    fn ops_concurrent_write_and_read_returning_written_value_is_ok() {
        // write(1)[0,20] overlaps read→1[5,15]; linearization: write then read. ✓
        let history = [op(0, write(1), 0, 0, 20), op(1, read(), 1, 5, 15)];
        assert_eq!(check_operations(&Reg, &history, None), CheckResult::Ok);
    }

    #[test]
    fn ops_concurrent_write_and_read_returning_init_value_is_ok() {
        // write(1)[0,20] overlaps read→0[5,15]; linearization: read then write. ✓
        let history = [op(0, write(1), 0, 0, 20), op(1, read(), 0, 5, 15)];
        assert_eq!(check_operations(&Reg, &history, None), CheckResult::Ok);
    }

    #[test]
    fn ops_read_starts_after_write_completes_returning_stale_is_illegal() {
        // write(1) completes at t=10; read starts at t=12 — strictly after write.
        // Returning 0 violates real-time order (INV-LIN-02).
        let history = [op(0, write(1), 0, 0, 10), op(1, read(), 0, 12, 20)];
        assert_eq!(check_operations(&Reg, &history, None), CheckResult::Illegal);
    }

    #[test]
    fn ops_instantaneous_op_is_ok() {
        // call == return_time: well-formedness guard allows call ≤ return_time.
        assert_eq!(
            check_operations(&Reg, &[op(0, write(7), 0, 5, 5)], None),
            CheckResult::Ok
        );
    }

    #[test]
    fn ops_multiple_reads_all_return_init_before_any_write_is_ok() {
        let history = [
            op(0, read(), 0, 0, 10),
            op(1, read(), 0, 2, 8),
            op(2, read(), 0, 4, 6),
        ];
        assert_eq!(check_operations(&Reg, &history, None), CheckResult::Ok);
    }

    #[test]
    fn ops_two_sequential_writes_then_wrong_read_is_illegal() {
        // write(1), write(2), read→1: last write was 2 so read must return 2.
        let history = [
            op(0, write(1), 0, 0, 10),
            op(1, write(2), 0, 11, 20),
            op(2, read(), 1, 21, 30),
        ];
        assert_eq!(check_operations(&Reg, &history, None), CheckResult::Illegal);
    }

    #[test]
    fn ops_cache_pruning_does_not_cause_false_illegal() {
        // Two identical writes reach the same (bitset, state) from two DFS paths.
        // The cache must not prune a valid unexplored path (INV-LIN-04).
        let history = [
            op(0, write(1), 0, 0, 20),
            op(1, write(1), 0, 5, 15),
            op(2, read(), 1, 25, 35),
        ];
        assert_eq!(check_operations(&Reg, &history, None), CheckResult::Ok);
    }

    #[test]
    fn ops_backtracking_finds_valid_ordering_after_failed_attempts() {
        // write(1)[0,30], write(2)[5,20], read→1[25,35].
        // DFS first tries linearize A(write 1)→B(write 2)→C(read→1 fails, state=2).
        // Backtracks; tries B(write 2)→A(write 1)→C(read→1, state=1 ✓).
        // Exercises full backtrack and unlift symmetry for two operations.
        let history = [
            op(0, write(1), 0, 0, 30),
            op(1, write(2), 0, 5, 20),
            op(2, read(), 1, 25, 35),
        ];
        assert_eq!(check_operations(&Reg, &history, None), CheckResult::Ok);
    }

    // -----------------------------------------------------------------------
    // check_events
    // -----------------------------------------------------------------------

    #[test]
    fn events_empty_history_is_ok() {
        let events: Vec<Event<RegInput, i32>> = vec![];
        assert_eq!(check_events(&Reg, &events, None), CheckResult::Ok);
    }

    #[test]
    fn events_single_write_is_ok() {
        let events = [call_ev(0, write(42)), ret_ev(0, 0)];
        assert_eq!(check_events(&Reg, &events, None), CheckResult::Ok);
    }

    #[test]
    fn events_single_read_returning_init_value_is_ok() {
        let events = [call_ev(0, read()), ret_ev(0, 0)];
        assert_eq!(check_events(&Reg, &events, None), CheckResult::Ok);
    }

    #[test]
    fn events_single_read_returning_wrong_value_is_illegal() {
        let events = [call_ev(0, read()), ret_ev(0, 99)];
        assert_eq!(check_events(&Reg, &events, None), CheckResult::Illegal);
    }

    #[test]
    fn events_sequential_write_then_correct_read_is_ok() {
        // Slice order = time: t=0 call_write, t=1 ret_write, t=2 call_read, t=3 ret_read→5.
        let events = [
            call_ev(0, write(5)),
            ret_ev(0, 0),
            call_ev(1, read()),
            ret_ev(1, 5),
        ];
        assert_eq!(check_events(&Reg, &events, None), CheckResult::Ok);
    }

    #[test]
    fn events_sequential_read_after_write_returning_stale_value_is_illegal() {
        let events = [
            call_ev(0, write(5)),
            ret_ev(0, 0),
            call_ev(1, read()),
            ret_ev(1, 0), // should be 5
        ];
        assert_eq!(check_events(&Reg, &events, None), CheckResult::Illegal);
    }

    #[test]
    fn events_concurrent_write_and_read_returning_written_value_is_ok() {
        // Interleaved: call_w, call_r, ret_r→1, ret_w.
        // Valid linearization: write(1) then read→1. ✓
        let events = [
            call_ev(0, write(1)),
            call_ev(1, read()),
            ret_ev(1, 1),
            ret_ev(0, 0),
        ];
        assert_eq!(check_events(&Reg, &events, None), CheckResult::Ok);
    }

    #[test]
    fn events_concurrent_write_and_read_returning_init_value_is_ok() {
        // Interleaved: call_w, call_r, ret_r→0, ret_w.
        // Valid linearization: read→0 then write(1). ✓
        let events = [
            call_ev(0, write(1)),
            call_ev(1, read()),
            ret_ev(1, 0),
            ret_ev(0, 0),
        ];
        assert_eq!(check_events(&Reg, &events, None), CheckResult::Ok);
    }

    #[test]
    fn events_sequential_read_after_completed_write_returning_stale_is_illegal() {
        // write(1) fully completes before read starts.
        // read returning 0 after write(1) has no valid linearization.
        let events = [
            call_ev(0, write(1)),
            ret_ev(0, 0),
            call_ev(1, read()),
            ret_ev(1, 0),
        ];
        assert_eq!(check_events(&Reg, &events, None), CheckResult::Illegal);
    }

    #[test]
    fn events_noncontiguous_ids_produce_same_result_as_contiguous_ids() {
        // IDs 100 and 999 represent the same logical sequential history as 0 and 1.
        // renumber() must normalize both to the same DFS problem.
        let contiguous = [
            call_ev(0, write(5)),
            ret_ev(0, 0),
            call_ev(1, read()),
            ret_ev(1, 5),
        ];
        let noncontiguous = [
            Event {
                client_id: 0,
                kind: EventKind::Call,
                input: Some(write(5)),
                output: None,
                id: 100,
            },
            Event {
                client_id: 0,
                kind: EventKind::Return,
                input: None,
                output: Some(0),
                id: 100,
            },
            Event {
                client_id: 1,
                kind: EventKind::Call,
                input: Some(read()),
                output: None,
                id: 999,
            },
            Event {
                client_id: 1,
                kind: EventKind::Return,
                input: None,
                output: Some(5),
                id: 999,
            },
        ];
        assert_eq!(check_events(&Reg, &contiguous, None), CheckResult::Ok);
        assert_eq!(check_events(&Reg, &noncontiguous, None), CheckResult::Ok);
    }

    #[test]
    fn events_agree_with_operations_on_linearizable_history() {
        // write(1)[0,10] overlaps read→1[5,15]. Both APIs must return Ok.
        // Equivalent event slice (time-sorted): call_w(t=0), call_r(t=5),
        //   ret_w(t=10), ret_r(t=15) → encoded as indices 0,1,2,3.
        let ops = [op(0, write(1), 0, 0, 10), op(1, read(), 1, 5, 15)];
        let events = [
            call_ev(0, write(1)),
            call_ev(1, read()),
            ret_ev(0, 0),
            ret_ev(1, 1),
        ];
        assert_eq!(check_operations(&Reg, &ops, None), CheckResult::Ok);
        assert_eq!(check_events(&Reg, &events, None), CheckResult::Ok);
    }

    #[test]
    fn events_agree_with_operations_on_illegal_history() {
        // write(1)[0,10], read→0[12,20]: non-overlapping, stale read.
        let ops = [op(0, write(1), 0, 0, 10), op(1, read(), 0, 12, 20)];
        let events = [
            call_ev(0, write(1)),
            ret_ev(0, 0),
            call_ev(1, read()),
            ret_ev(1, 0),
        ];
        assert_eq!(check_operations(&Reg, &ops, None), CheckResult::Illegal);
        assert_eq!(check_events(&Reg, &events, None), CheckResult::Illegal);
    }

    #[test]
    fn events_backtracking_finds_valid_ordering_after_failed_attempts() {
        // Three overlapping ops: call_w1, call_w2, call_r, ret_r→1, ret_w2, ret_w1.
        // DFS first tries w1(state=1) then w2(state=2) then r→1 (1≠2, fails).
        // Backtracks to try w2(state=2) then w1(state=1) then r→1 (1==1, ✓).
        let events = [
            call_ev(0, write(1)),
            call_ev(1, write(2)),
            call_ev(2, read()),
            ret_ev(2, 1),
            ret_ev(1, 0),
            ret_ev(0, 0),
        ];
        assert_eq!(check_events(&Reg, &events, None), CheckResult::Ok);
    }

    // -----------------------------------------------------------------------
    // Timeout behaviour
    // -----------------------------------------------------------------------

    #[test]
    fn timeout_zero_duration_returns_unknown_or_definitive() {
        // A zero-length timeout may fire before the DFS even starts.
        // The result must be one of the three valid variants — never a panic.
        let history = [op(0, write(1), 0, 0, 10), op(1, read(), 1, 5, 15)];
        let result = check_operations(&Reg, &history, Some(Duration::ZERO));
        assert!(
            matches!(result, CheckResult::Ok | CheckResult::Unknown),
            "expected Ok or Unknown for a zero-duration timeout, got {:?}",
            result
        );
    }

    #[test]
    fn timeout_very_long_does_not_affect_result() {
        // A timeout far in the future must not influence the result at all:
        // the DFS finishes before the timer fires.
        let history = [op(0, write(1), 0, 0, 10), op(1, read(), 1, 5, 15)];
        assert_eq!(
            check_operations(&Reg, &history, Some(Duration::from_secs(60))),
            CheckResult::Ok
        );
    }

    #[test]
    fn timeout_very_long_does_not_affect_illegal_result() {
        // Same guarantee for an illegal history.
        let history = [op(0, write(1), 0, 0, 10), op(1, read(), 0, 12, 20)];
        assert_eq!(
            check_operations(&Reg, &history, Some(Duration::from_secs(60))),
            CheckResult::Illegal
        );
    }

    #[test]
    fn timeout_none_matches_none_no_timeout() {
        // timeout=None and a very long timeout must agree on a known-Ok history.
        let history = [op(0, write(5), 0, 0, 10), op(1, read(), 5, 11, 20)];
        assert_eq!(
            check_operations(&Reg, &history, None),
            check_operations(&Reg, &history, Some(Duration::from_secs(60)))
        );
    }

    #[test]
    fn timeout_events_very_long_does_not_affect_result() {
        let events = [
            call_ev(0, write(1)),
            call_ev(1, read()),
            ret_ev(1, 1),
            ret_ev(0, 0),
        ];
        assert_eq!(
            check_events(&Reg, &events, Some(Duration::from_secs(60))),
            CheckResult::Ok
        );
    }

    // -----------------------------------------------------------------------
    // check_operations_parallel / check_parallel_rayon (unit tests)
    //
    // These tests directly exercise `check_operations_parallel`, which
    // dispatches to `check_parallel_rayon` internally.  They complement the
    // property tests in tests/property_tests.rs by using a concrete
    // multi-partition history so that the partition splitting and rayon
    // dispatch path can be inspected deterministically.
    //
    // Model: KvModel — a two-type key/value store that partitions by key.
    //   State:  HashMap<u8, i32>  (key → value; missing key reads as 0)
    //   Input:  KvInput { key, is_write, value }
    //   Output: i32
    //   step:   write always succeeds; read succeeds iff output == stored value
    //   partition: groups operation indices by key → each key is independent
    // -----------------------------------------------------------------------

    mod partition_tests {
        use super::*;
        use std::collections::HashMap;

        #[derive(Clone)]
        struct KvModel;

        #[derive(Clone, Debug, PartialEq)]
        struct KvInput {
            key: u8,
            is_write: bool,
            value: i32,
        }

        impl Model for KvModel {
            type State = HashMap<u8, i32>;
            type Input = KvInput;
            type Output = i32;

            fn init(&self) -> Self::State {
                HashMap::new()
            }

            fn step(
                &self,
                state: &Self::State,
                input: &KvInput,
                output: &i32,
            ) -> Option<Self::State> {
                let mut next = state.clone();
                if input.is_write {
                    next.insert(input.key, input.value);
                    Some(next)
                } else {
                    let stored = *state.get(&input.key).unwrap_or(&0);
                    if *output == stored { Some(next) } else { None }
                }
            }

            fn partition(&self, history: &[Operation<KvInput, i32>]) -> Option<Vec<Vec<usize>>> {
                let mut by_key: HashMap<u8, Vec<usize>> = HashMap::new();
                for (i, op) in history.iter().enumerate() {
                    by_key.entry(op.input.key).or_default().push(i);
                }
                Some(by_key.into_values().collect())
            }
        }

        fn kv_write(key: u8, value: i32) -> KvInput {
            KvInput {
                key,
                is_write: true,
                value,
            }
        }
        fn kv_read(key: u8) -> KvInput {
            KvInput {
                key,
                is_write: false,
                value: 0,
            }
        }

        fn kv_op(
            id: u64,
            input: KvInput,
            output: i32,
            call: u64,
            ret: u64,
        ) -> Operation<KvInput, i32> {
            Operation {
                client_id: id,
                input,
                output,
                call,
                return_time: ret,
            }
        }

        #[test]
        fn two_partition_ok_history() {
            // Two independent keys, each with a sequential write-then-read.
            // partition() splits into 2 groups checked concurrently by rayon.
            let history = [
                kv_op(0, kv_write(0, 1), 0, 0, 10),
                kv_op(1, kv_read(0), 1, 11, 20),
                kv_op(2, kv_write(1, 5), 0, 0, 10),
                kv_op(3, kv_read(1), 5, 11, 20),
            ];
            assert_eq!(check_operations(&KvModel, &history, None), CheckResult::Ok);
        }

        #[test]
        fn two_partition_illegal_history() {
            // Key 0: write(1) then read→0 (stale read, illegal).
            // Key 1: write(5) then read→5 (ok).
            // One illegal partition must propagate Illegal for the whole check.
            let history = [
                kv_op(0, kv_write(0, 1), 0, 0, 10),
                kv_op(1, kv_read(0), 0, 11, 20), // stale; should be 1
                kv_op(2, kv_write(1, 5), 0, 0, 10),
                kv_op(3, kv_read(1), 5, 11, 20),
            ];
            assert_eq!(
                check_operations(&KvModel, &history, None),
                CheckResult::Illegal
            );
        }

        #[test]
        fn three_partitions_all_ok() {
            // Three independent keys; exercises rayon dispatch across 3 partitions.
            let history = [
                kv_op(0, kv_write(0, 1), 0, 0, 10),
                kv_op(1, kv_read(0), 1, 11, 20),
                kv_op(2, kv_write(1, 2), 0, 0, 10),
                kv_op(3, kv_read(1), 2, 11, 20),
                kv_op(4, kv_write(2, 3), 0, 0, 10),
                kv_op(5, kv_read(2), 3, 11, 20),
            ];
            assert_eq!(check_operations(&KvModel, &history, None), CheckResult::Ok);
        }
    }

    // -----------------------------------------------------------------------
    // check_events with partition_events (gap 5)
    //
    // KvModel below is the first model in this file that implements
    // partition_events, closing the coverage gap on the event-based
    // partition path in check_events.
    //
    // partition_events strategy:
    //   Call events carry the key in their input field.
    //   Return events share the same `id` as their matching Call.
    //   First pass: build id → key from Call events.
    //   Second pass: assign every event index to its key's partition.
    // -----------------------------------------------------------------------

    mod events_partition_tests {
        use super::*;
        use std::collections::HashMap;

        #[derive(Clone)]
        struct KvModel;

        #[derive(Clone, Debug, PartialEq)]
        struct KvInput {
            key: u8,
            is_write: bool,
            value: i32,
        }

        impl Model for KvModel {
            type State = HashMap<u8, i32>;
            type Input = KvInput;
            type Output = i32;

            fn init(&self) -> Self::State {
                HashMap::new()
            }

            fn step(
                &self,
                state: &Self::State,
                input: &KvInput,
                output: &i32,
            ) -> Option<Self::State> {
                let mut next = state.clone();
                if input.is_write {
                    next.insert(input.key, input.value);
                    Some(next)
                } else {
                    let stored = *state.get(&input.key).unwrap_or(&0);
                    if *output == stored { Some(next) } else { None }
                }
            }

            fn partition_events(&self, history: &[Event<KvInput, i32>]) -> Option<Vec<Vec<usize>>> {
                // First pass: map event id → key from Call events.
                let mut id_to_key: HashMap<u64, u8> = HashMap::new();
                for ev in history {
                    if let (EventKind::Call, Some(input)) = (&ev.kind, &ev.input) {
                        id_to_key.insert(ev.id, input.key);
                    }
                }
                // Second pass: group each event index into its key's partition.
                let mut by_key: HashMap<u8, Vec<usize>> = HashMap::new();
                for (i, ev) in history.iter().enumerate() {
                    if let Some(&key) = id_to_key.get(&ev.id) {
                        by_key.entry(key).or_default().push(i);
                    }
                }
                Some(by_key.into_values().collect())
            }
        }

        fn kv_call(id: u64, key: u8, is_write: bool, value: i32) -> Event<KvInput, i32> {
            Event {
                client_id: id,
                kind: EventKind::Call,
                input: Some(KvInput {
                    key,
                    is_write,
                    value,
                }),
                output: None,
                id,
            }
        }
        fn kv_ret(id: u64, output: i32) -> Event<KvInput, i32> {
            Event {
                client_id: id,
                kind: EventKind::Return,
                input: None,
                output: Some(output),
                id,
            }
        }

        #[test]
        fn check_events_partition_two_keys_ok() {
            // Two independent sequential sub-histories, one per key.
            // partition_events must split them into 2 groups; both are Ok.
            let history = [
                kv_call(0, 0, true, 1),
                kv_ret(0, 0), // key 0: write(1)
                kv_call(1, 0, false, 0),
                kv_ret(1, 1), // key 0: read→1
                kv_call(2, 1, true, 5),
                kv_ret(2, 0), // key 1: write(5)
                kv_call(3, 1, false, 0),
                kv_ret(3, 5), // key 1: read→5
            ];
            assert_eq!(check_events(&KvModel, &history, None), CheckResult::Ok);
        }

        #[test]
        fn check_events_partition_detects_illegal_in_one_key() {
            // key 0: write(1) then read→0 (stale — illegal)
            // key 1: write(5) then read→5 (ok)
            // The illegal partition must propagate Illegal for the whole check.
            let history = [
                kv_call(0, 0, true, 1),
                kv_ret(0, 0),
                kv_call(1, 0, false, 0),
                kv_ret(1, 0), // stale read; should be 1
                kv_call(2, 1, true, 5),
                kv_ret(2, 0),
                kv_call(3, 1, false, 0),
                kv_ret(3, 5),
            ];
            assert_eq!(check_events(&KvModel, &history, None), CheckResult::Illegal);
        }

        #[test]
        fn check_events_partition_concurrent_writes_ok() {
            // Two writes on different keys overlap in time (interleaved events).
            // Each key's sub-history is independently linearizable.
            let history = [
                kv_call(0, 0, true, 1), // key 0 call
                kv_call(1, 1, true, 5), // key 1 call (concurrent)
                kv_ret(0, 0),           // key 0 return
                kv_ret(1, 0),           // key 1 return
            ];
            assert_eq!(check_events(&KvModel, &history, None), CheckResult::Ok);
        }
    }

    // -----------------------------------------------------------------------
    // Definitive Unknown result via artificially slow model (gap 7)
    //
    // SlowModel.step() sleeps STEP_MS milliseconds, ensuring the timer
    // (set to TIMER_MS << STEP_MS) fires before the DFS iteration that
    // called step() can loop back and observe cursor=None.
    //
    // Why this is reliable:
    //   - The timer fires at TIMER_MS (~2 ms).
    //   - step() returns at STEP_MS (~50 ms) → kill is already true.
    //   - Even if cursor happens to be None at that point,
    //     to_check_result() checks timed_out BEFORE ok, so Unknown is
    //     returned regardless of the DFS's final boolean.
    // -----------------------------------------------------------------------

    mod timeout_unknown_tests {
        use super::*;

        const STEP_MS: u64 = 50;
        const TIMER_MS: u64 = 2;

        #[derive(Clone)]
        struct SlowModel;

        impl Model for SlowModel {
            type State = ();
            type Input = ();
            type Output = ();

            fn init(&self) -> () {
                ()
            }

            fn step(&self, _state: &(), _input: &(), _output: &()) -> Option<()> {
                std::thread::sleep(Duration::from_millis(STEP_MS));
                Some(())
            }
        }

        fn slow_op(id: u64) -> Operation<(), ()> {
            Operation {
                client_id: id,
                input: (),
                output: (),
                call: id,
                return_time: id + 1,
            }
        }

        #[test]
        fn timeout_short_duration_returns_unknown() {
            // Timer fires at 2 ms; step sleeps 50 ms.
            // The kill + timed_out flags are set long before DFS can finish.
            let history = [slow_op(0)];
            assert_eq!(
                check_operations(&SlowModel, &history, Some(Duration::from_millis(TIMER_MS))),
                CheckResult::Unknown
            );
        }

        #[test]
        fn timeout_short_duration_events_returns_unknown() {
            // Same guarantee via check_events.
            let history = [
                Event {
                    client_id: 0,
                    kind: EventKind::Call,
                    input: Some(()),
                    output: None,
                    id: 0,
                },
                Event {
                    client_id: 0,
                    kind: EventKind::Return,
                    input: None,
                    output: Some(()),
                    id: 0,
                },
            ];
            assert_eq!(
                check_events(&SlowModel, &history, Some(Duration::from_millis(TIMER_MS))),
                CheckResult::Unknown
            );
        }
    }

    // -----------------------------------------------------------------------
    // to_check_result — all four (ok, timed_out, definitive_illegal) cases
    //
    // These tests pin the priority contract that was fixed in the CoPilot
    // review: Illegal must take priority over Unknown when a partition
    // completed its full DFS and proved non-linearizability, even if the
    // timer also fired.
    // -----------------------------------------------------------------------

    mod to_check_result_tests {
        use super::*;

        #[test]
        fn illegal_takes_priority_over_unknown() {
            // Core guarantee of the fix: a definitive Illegal beats a timeout.
            let timed_out = AtomicBool::new(true);
            let definitive_illegal = AtomicBool::new(true);
            assert_eq!(
                to_check_result(false, &timed_out, &definitive_illegal),
                CheckResult::Illegal,
            );
        }

        #[test]
        fn unknown_when_only_timer_fired() {
            // Timer fired but no partition finished its full DFS.
            let timed_out = AtomicBool::new(true);
            let definitive_illegal = AtomicBool::new(false);
            assert_eq!(
                to_check_result(false, &timed_out, &definitive_illegal),
                CheckResult::Unknown,
            );
        }

        #[test]
        fn ok_when_dfs_completed_cleanly() {
            let timed_out = AtomicBool::new(false);
            let definitive_illegal = AtomicBool::new(false);
            assert_eq!(
                to_check_result(true, &timed_out, &definitive_illegal),
                CheckResult::Ok,
            );
        }

        #[test]
        fn illegal_when_dfs_finished_no_timeout() {
            // !ok, no timer, no concurrent kill — straightforward Illegal.
            let timed_out = AtomicBool::new(false);
            let definitive_illegal = AtomicBool::new(false);
            assert_eq!(
                to_check_result(false, &timed_out, &definitive_illegal),
                CheckResult::Illegal,
            );
        }
    }
}
