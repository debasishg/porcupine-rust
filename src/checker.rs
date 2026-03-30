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

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::bitset::Bitset;
use crate::invariants::{assert_partition_independent, assert_well_formed};
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

#[derive(Clone)]
struct Entry<I, O> {
    id:    usize, // operation id (0-indexed); call and return share the same id
    time:  i64,
    value: EntryValue<I, O>,
}

/// Flatten a slice of `Operation`s into a sorted Vec of `Entry` pairs.
/// Calls precede returns at equal timestamps (mirrors Go `byTime` sort).
fn make_entries<I: Clone, O: Clone>(ops: &[Operation<I, O>]) -> Vec<Entry<I, O>> {
    let mut entries = Vec::with_capacity(ops.len() * 2);
    for (id, op) in ops.iter().enumerate() {
        entries.push(Entry { id, time: op.call as i64,        value: EntryValue::Call(op.input.clone()) });
        entries.push(Entry { id, time: op.return_time as i64, value: EntryValue::Return(op.output.clone()) });
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

/// Renumber `Event` IDs to be contiguous starting at 0.
fn renumber<I: Clone, O: Clone>(events: &[Event<I, O>]) -> Vec<Event<I, O>> {
    let mut out = Vec::with_capacity(events.len());
    let mut map: HashMap<u64, u64> = HashMap::new();
    let mut next_id = 0u64;
    for ev in events {
        let new_id = *map.entry(ev.id).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
        out.push(Event { id: new_id, ..ev.clone() });
    }
    out
}

/// Convert a renumbered slice of `Event`s into `Entry`s (index as time).
fn convert_entries<I: Clone, O: Clone>(events: &[Event<I, O>]) -> Vec<Entry<I, O>> {
    events.iter().enumerate().map(|(i, ev)| {
        let value = match ev.kind {
            EventKind::Call   => EntryValue::Call(ev.input.clone().expect("Call event must have input")),
            EventKind::Return => EntryValue::Return(ev.output.clone().expect("Return event must have output")),
        };
        Entry { id: ev.id as usize, time: i as i64, value }
    }).collect()
}

// ---------------------------------------------------------------------------
// Index-based doubly-linked list (NodeArena)
// ---------------------------------------------------------------------------

// Sentinel HEAD is always at index 0.
// All real nodes occupy indices 1 ..= 2n.
//
// `value` is `None` only for the sentinel; always `Some` for real nodes.
struct Node<I, O> {
    value:     Option<EntryValue<I, O>>,
    match_idx: Option<usize>, // Some(ret_idx) for call nodes, None for return/sentinel
    id:        usize,
    prev:      usize,         // index of previous node (sentinel = 0)
    next:      Option<usize>, // None at end of list
}

struct NodeArena<I, O> {
    nodes: Vec<Node<I, O>>,
}

impl<I, O> NodeArena<I, O> {
    /// Build the arena from a sorted entry list.
    fn from_entries(entries: Vec<Entry<I, O>>) -> Self {
        let n = entries.len();
        let mut arena_nodes: Vec<Node<I, O>> = Vec::with_capacity(n + 1);

        // Sentinel at index 0 — value is None (never accessed in DFS).
        arena_nodes.push(Node {
            value:     None,
            match_idx: None,
            id:        usize::MAX,
            prev:      0,
            next:      None,
        });

        // Track which node index holds the return for each operation id.
        let mut return_idx: HashMap<usize, usize> = HashMap::new();

        // Allocate a slot for each entry.
        for (i, entry) in entries.into_iter().enumerate() {
            let node_idx = i + 1; // 1-indexed
            if matches!(entry.value, EntryValue::Return(_)) {
                return_idx.insert(entry.id, node_idx);
            }
            arena_nodes.push(Node {
                value:     Some(entry.value),
                match_idx: None, // filled in next pass
                id:        entry.id,
                prev:      0,
                next:      None,
            });
        }

        // Fill match_idx for call nodes.
        for i in 1..=n {
            if matches!(arena_nodes[i].value, Some(EntryValue::Call(_))) {
                let op_id = arena_nodes[i].id;
                if let Some(&ret_i) = return_idx.get(&op_id) {
                    arena_nodes[i].match_idx = Some(ret_i);
                }
            }
        }

        // Link nodes in order: sentinel → 1 → 2 → … → n
        for i in 1..=n {
            arena_nodes[i].prev = i - 1;
            if i < n {
                arena_nodes[i].next = Some(i + 1);
            }
        }
        arena_nodes[0].next = if n > 0 { Some(1) } else { None };

        NodeArena { nodes: arena_nodes }
    }

    /// Index of the first live node after sentinel HEAD.
    fn head_next(&self) -> Option<usize> {
        self.nodes[0].next
    }

    /// Remove `call_idx` and its matched return node from the live list.
    fn lift(&mut self, call_idx: usize) {
        let match_idx = self.nodes[call_idx].match_idx.unwrap();

        // Unlink call node.
        let call_prev = self.nodes[call_idx].prev;
        let call_next = self.nodes[call_idx].next;
        self.nodes[call_prev].next = call_next;
        if let Some(cn) = call_next {
            self.nodes[cn].prev = call_prev;
        }

        // Unlink return node.
        let ret_prev = self.nodes[match_idx].prev;
        let ret_next = self.nodes[match_idx].next;
        self.nodes[ret_prev].next = ret_next;
        if let Some(rn) = ret_next {
            self.nodes[rn].prev = ret_prev;
        }
    }

    /// Re-insert `call_idx` and its matched return node back into the live list.
    fn unlift(&mut self, call_idx: usize) {
        let match_idx = self.nodes[call_idx].match_idx.unwrap();

        // Re-link return node.
        let ret_prev = self.nodes[match_idx].prev;
        let ret_next = self.nodes[match_idx].next;
        self.nodes[ret_prev].next = Some(match_idx);
        if let Some(rn) = ret_next {
            self.nodes[rn].prev = match_idx;
        }

        // Re-link call node.
        let call_prev = self.nodes[call_idx].prev;
        let call_next = self.nodes[call_idx].next;
        self.nodes[call_prev].next = Some(call_idx);
        if let Some(cn) = call_next {
            self.nodes[cn].prev = call_idx;
        }
    }
}

// ---------------------------------------------------------------------------
// DFS cache
// ---------------------------------------------------------------------------

struct CacheEntry<S> {
    linearized: Bitset,
    state:      S,
}

fn cache_contains<S: PartialEq>(cache: &HashMap<u64, Vec<CacheEntry<S>>>, hash: u64, bitset: &Bitset, state: &S) -> bool {
    if let Some(entries) = cache.get(&hash) {
        for e in entries {
            if e.linearized.equals(bitset) && &e.state == state {
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
    node_idx: usize, // index of the call node that was linearized
    state:    S,     // model state *before* this linearization step
}

// ---------------------------------------------------------------------------
// check_single — the core DFS
// ---------------------------------------------------------------------------

fn check_single<M: Model>(
    model:   &M,
    entries: Vec<Entry<M::Input, M::Output>>,
    kill:    &AtomicBool,
) -> bool {
    if entries.is_empty() {
        return true;
    }

    let n_ops = entries.len() / 2; // number of operations
    let mut arena = NodeArena::from_entries(entries);
    let mut linearized = Bitset::new(n_ops);
    let mut cache: HashMap<u64, Vec<CacheEntry<M::State>>> = HashMap::new();
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
                match arena.nodes[idx].match_idx {
                    Some(ret_idx) => {
                        // This is a call node. Try to linearize it.
                        // INV-HIST-03: the live list is always time-sorted, and we restart
                        // from head_next() after each lift, so the first call node we visit
                        // is always the minimal one (no unlinearized op has an earlier call).
                        let op_id = arena.nodes[idx].id;
                        let (input, output) = match (
                            arena.nodes[idx].value.as_ref().unwrap(),
                            arena.nodes[ret_idx].value.as_ref().unwrap(),
                        ) {
                            (EntryValue::Call(i), EntryValue::Return(o)) => (i, o),
                            _ => unreachable!("match_idx must point to a Return node"),
                        };

                        if let Some(next_state) = model.step(&state, input, output) {
                            let mut new_linearized = linearized.clone();
                            new_linearized.set(op_id);
                            let h = new_linearized.hash();

                            if !cache_contains(&cache, h, &new_linearized, &next_state) {
                                // INV-LIN-04: new (bitset, state) pair — safe to cache.
                                cache.entry(h).or_default().push(CacheEntry {
                                    linearized: new_linearized,
                                    state:      next_state.clone(),
                                });
                                calls.push(CallFrame { node_idx: idx, state: state.clone() });
                                state = next_state;
                                linearized.set(op_id);
                                arena.lift(idx);
                                cursor = arena.head_next();
                            } else {
                                // Already explored this (bitset, state) — skip.
                                cursor = arena.nodes[idx].next;
                            }
                        } else {
                            // Model rejected this linearization point — try next.
                            cursor = arena.nodes[idx].next;
                        }
                    }
                    None => {
                        // This is a return node with no linearized call preceding it.
                        // We're stuck — backtrack.
                        if calls.is_empty() {
                            return false;
                        }
                        let frame = calls.pop().unwrap();
                        let call_op_id = arena.nodes[frame.node_idx].id;
                        state = frame.state;
                        linearized.clear(call_op_id);
                        arena.unlift(frame.node_idx);
                        // Advance past the restored call node.
                        cursor = arena.nodes[frame.node_idx].next;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// check_parallel — run one check_single per partition
// ---------------------------------------------------------------------------

fn check_parallel<M: Model>(
    model:      &M,
    partitions: Vec<Vec<Entry<M::Input, M::Output>>>,
) -> CheckResult {
    if partitions.is_empty() {
        return CheckResult::Ok;
    }

    let kill = AtomicBool::new(false);

    for partition in partitions {
        if !check_single(model, partition, &kill) {
            kill.store(true, Ordering::Relaxed);
            return CheckResult::Illegal;
        }
    }

    CheckResult::Ok
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Check an operation-based history for linearizability.
pub fn check_operations<M: Model>(model: &M, history: &[Operation<M::Input, M::Output>]) -> CheckResult {
    // INV-HIST-01
    assert_well_formed!(history);

    let partitions: Vec<Vec<Entry<M::Input, M::Output>>> =
        if let Some(parts) = model.partition(history) {
            // INV-LIN-03
            assert_partition_independent!(parts);
            parts.iter()
                .map(|indices| make_entries(&indices.iter().map(|&i| history[i].clone()).collect::<Vec<_>>()))
                .collect()
        } else {
            vec![make_entries(history)]
        };

    check_parallel(model, partitions)
}

/// Check an event-based history for linearizability.
pub fn check_events<M: Model>(model: &M, history: &[Event<M::Input, M::Output>]) -> CheckResult {
    let partitions: Vec<Vec<Entry<M::Input, M::Output>>> =
        if let Some(parts) = model.partition_events(history) {
            assert_partition_independent!(parts);
            parts.iter()
                .map(|indices| {
                    let sub: Vec<Event<M::Input, M::Output>> = indices.iter().map(|&i| history[i].clone()).collect();
                    convert_entries(&renumber(&sub))
                })
                .collect()
        } else {
            vec![convert_entries(&renumber(history))]
        };

    check_parallel(model, partitions)
}
