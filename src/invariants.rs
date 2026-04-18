// Runtime invariant assertions for porcupine-rust.
//
// Every macro here corresponds to an `INV-*` identifier in `docs/spec.md`.
// Run `/spec-sync` to verify that no INV-* ID exists in one place but not the other.

// ---------------------------------------------------------------------------
// INV-HIST-01: Well-Formed History
// ---------------------------------------------------------------------------

/// Assert that a slice of operations is well-formed:
/// every op has `call ≤ return_time` and non-negative timestamps.
///
/// # INV-HIST-01
macro_rules! assert_well_formed {
    ($ops:expr) => {
        #[cfg(debug_assertions)]
        for op in $ops.iter() {
            debug_assert!(
                op.call <= op.return_time,
                "INV-HIST-01: op {} has call ({}) > return_time ({})",
                op.client_id,
                op.call,
                op.return_time
            );
        }
    };
}

// ---------------------------------------------------------------------------
// INV-HIST-03: Minimal-Call Frontier
// ---------------------------------------------------------------------------

/// Assert that `op` is a minimal call: no unlinearized operation has a strictly
/// earlier call timestamp.
///
/// # INV-HIST-03
///
/// Note: this macro is designed for use with `Operation` slices. In the DFS
/// (`checker.rs`) INV-HIST-03 is enforced structurally — `head_next()` always
/// returns the first live call in time-sorted order, so minimality is guaranteed
/// by construction rather than by an explicit assertion.
#[allow(unused_macros)]
macro_rules! assert_minimal_call {
    ($op:expr, $all_ops:expr, $linearized_ids:expr) => {
        #[cfg(debug_assertions)]
        {
            let op_call = $op.call;
            for other in $all_ops.iter() {
                if !$linearized_ids.contains(&other.client_id) && other.client_id != $op.client_id {
                    debug_assert!(
                        other.call >= op_call,
                        "INV-HIST-03: op {} (call={}) is not minimal; op {} (call={}) precedes it",
                        $op.client_id,
                        op_call,
                        other.client_id,
                        other.call,
                    );
                }
            }
        }
    };
}

// ---------------------------------------------------------------------------
// INV-LIN-03: P-Compositionality (partition independence)
// ---------------------------------------------------------------------------

/// Assert that partitions produced by the model do not share any operation indices,
/// ensuring sub-histories are truly independent.
///
/// Subsumed by `assert_partition_covers_ops!` and `assert_partition_events_paired!`
/// which also check full coverage and bounds. Kept for spec traceability.
///
/// # INV-LIN-03
#[allow(unused_macros)]
macro_rules! assert_partition_independent {
    ($partitions:expr) => {
        #[cfg(debug_assertions)]
        {
            let mut seen = std::collections::HashSet::new();
            for partition in $partitions.iter() {
                for &idx in partition.iter() {
                    debug_assert!(
                        seen.insert(idx),
                        "INV-LIN-03: operation index {} appears in more than one partition",
                        idx
                    );
                }
            }
        }
    };
}

// ---------------------------------------------------------------------------
// INV-LIN-03b: Partition Full Coverage (operations)
// ---------------------------------------------------------------------------

/// Assert that partitions collectively cover every operation in the history.
/// A buggy partitioner that omits an operation would make the checker silently
/// skip work and potentially return `Ok` incorrectly.
///
/// # INV-LIN-03
macro_rules! assert_partition_covers_ops {
    ($partitions:expr, $history_len:expr) => {
        if cfg!(debug_assertions) {
            let mut seen = std::collections::HashSet::new();
            for partition in $partitions.iter() {
                for &idx in partition.iter() {
                    assert!(
                        idx < $history_len,
                        "INV-LIN-03: partition index {} is out of bounds (history length {})",
                        idx,
                        $history_len
                    );
                    assert!(
                        seen.insert(idx),
                        "INV-LIN-03: operation index {} appears in more than one partition",
                        idx
                    );
                }
            }
            assert!(
                seen.len() == $history_len,
                "INV-LIN-03: partitions cover {} operations but history has {} — \
                 {} operation(s) missing from all partitions",
                seen.len(),
                $history_len,
                $history_len - seen.len()
            );
        }
    };
}

// ---------------------------------------------------------------------------
// INV-LIN-03c: Partition Event Pair Integrity
// ---------------------------------------------------------------------------

/// Assert that event-based partitions keep call/return pairs together.
/// If a partitioner splits a pair across partitions, the per-partition DFS
/// sees a malformed history and can return spurious `Illegal`.
///
/// Also validates full coverage (every event index appears in exactly one
/// partition) and bounds.
///
/// # INV-LIN-03
macro_rules! assert_partition_events_paired {
    ($partitions:expr, $history:expr) => {
        if cfg!(debug_assertions) {
            let history_len = $history.len();
            // Build a map: event index → partition index
            let mut idx_to_part: std::collections::HashMap<usize, usize> =
                std::collections::HashMap::new();
            for (part_idx, partition) in $partitions.iter().enumerate() {
                for &ev_idx in partition.iter() {
                    assert!(
                        ev_idx < history_len,
                        "INV-LIN-03: event index {} is out of bounds (history length {})",
                        ev_idx,
                        history_len
                    );
                    let prev = idx_to_part.insert(ev_idx, part_idx);
                    assert!(
                        prev.is_none(),
                        "INV-LIN-03: event index {} appears in more than one partition",
                        ev_idx
                    );
                }
            }
            assert!(
                idx_to_part.len() == history_len,
                "INV-LIN-03: partitions cover {} events but history has {} — \
                 {} event(s) missing from all partitions",
                idx_to_part.len(),
                history_len,
                history_len - idx_to_part.len()
            );
            // For every event id, find both its Call and Return indices and
            // check they map to the same partition.
            let mut call_indices: std::collections::HashMap<u64, usize> =
                std::collections::HashMap::new();
            let mut return_indices: std::collections::HashMap<u64, usize> =
                std::collections::HashMap::new();
            for (pos, ev) in $history.iter().enumerate() {
                match ev.kind {
                    $crate::types::EventKind::Call => {
                        call_indices.insert(ev.id, pos);
                    }
                    $crate::types::EventKind::Return => {
                        return_indices.insert(ev.id, pos);
                    }
                }
            }
            for (&id, &call_pos) in &call_indices {
                if let Some(&ret_pos) = return_indices.get(&id) {
                    let call_part = idx_to_part[&call_pos];
                    let ret_part = idx_to_part[&ret_pos];
                    assert!(
                        call_part == ret_part,
                        "INV-LIN-03: event id={} has Call (index {}) in partition {} \
                         and Return (index {}) in partition {} — \
                         call/return pairs must not be split across partitions",
                        id, call_pos, call_part, ret_pos, ret_part
                    );
                }
            }
        }
    };
}

// ---------------------------------------------------------------------------
// INV-LIN-04: Cache Soundness
// ---------------------------------------------------------------------------

/// Assert that a cache hit is only used when the stored state equals the current
/// model state (same bitset key + same state → same result).
///
/// # INV-LIN-04
///
/// Note: in `checker.rs` the cache lookup already checks `state == cached_state`
/// via `PartialEq` inside `cache_contains`. This macro is available for any
/// future context where the check needs to be made explicitly at a call-site.
#[allow(unused_macros)]
macro_rules! assert_cache_sound {
    ($cached_state:expr, $current_state:expr) => {
        debug_assert!(
            $cached_state == $current_state,
            "INV-LIN-04: cache hit on matching bitset but states differ — cache key collision"
        );
    };
}

// ---------------------------------------------------------------------------
// INV-HIST-01 (event form): Well-Formed Event History
// ---------------------------------------------------------------------------

/// Assert that a slice of events is well-formed:
///  - every Call event has `input: Some(_)` and at most one occurrence per id,
///  - every Return event has `output: Some(_)` and at most one occurrence per id,
///  - every Call id has exactly one matching Return id (and vice versa),
///  - each Call appears strictly before its matching Return in the slice
///    (position ordering = time ordering for `check_events`).
///
/// # INV-HIST-01
macro_rules! assert_well_formed_events {
    ($events:expr) => {
        #[cfg(debug_assertions)]
        {
            // Track the slice position of the first occurrence of each id
            // as a Call or Return so we can check ordering and uniqueness.
            let mut call_pos: std::collections::HashMap<u64, usize> =
                std::collections::HashMap::new();
            let mut return_pos: std::collections::HashMap<u64, usize> =
                std::collections::HashMap::new();
            for (pos, ev) in $events.iter().enumerate() {
                match ev.kind {
                    $crate::types::EventKind::Call => {
                        debug_assert!(
                            ev.input.is_some(),
                            "INV-HIST-01: Call event id={} at position {} has input=None",
                            ev.id,
                            pos
                        );
                        debug_assert!(
                            call_pos.insert(ev.id, pos).is_none(),
                            "INV-HIST-01: id={} appears as Call more than once",
                            ev.id
                        );
                    }
                    $crate::types::EventKind::Return => {
                        debug_assert!(
                            ev.output.is_some(),
                            "INV-HIST-01: Return event id={} at position {} has output=None",
                            ev.id,
                            pos
                        );
                        debug_assert!(
                            return_pos.insert(ev.id, pos).is_none(),
                            "INV-HIST-01: id={} appears as Return more than once",
                            ev.id
                        );
                    }
                }
            }
            // Every Call must have a matching Return, and must precede it.
            for (&id, &c_pos) in &call_pos {
                match return_pos.get(&id) {
                    None => debug_assert!(
                        false,
                        "INV-HIST-01: Call event id={} at position {} has no matching Return",
                        id, c_pos
                    ),
                    Some(&r_pos) => debug_assert!(
                        c_pos < r_pos,
                        "INV-HIST-01: Call event id={} at position {} must precede \
                         its Return at position {}",
                        id,
                        c_pos,
                        r_pos
                    ),
                }
            }
            // Every Return must have a matching Call.
            for (&id, &r_pos) in &return_pos {
                debug_assert!(
                    call_pos.contains_key(&id),
                    "INV-HIST-01: Return event id={} at position {} has no matching Call",
                    id,
                    r_pos
                );
            }
        }
    };
}

// assert_cache_sound and assert_minimal_call are enforced structurally in
// checker.rs (see INV-HIST-03 and INV-LIN-04 notes on each macro). They are
// exported here for optional explicit use at future call-sites.
#[allow(unused_imports)]
pub(crate) use assert_cache_sound;
#[allow(unused_imports)]
pub(crate) use assert_minimal_call;
#[allow(unused_imports)]
pub(crate) use assert_partition_covers_ops;
#[allow(unused_imports)]
pub(crate) use assert_partition_events_paired;
#[allow(unused_imports)]
pub(crate) use assert_partition_independent;
pub(crate) use assert_well_formed;
pub(crate) use assert_well_formed_events;
