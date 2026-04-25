// Runtime invariant assertions for porcupine-rust.
//
// Every macro / function here corresponds to an `INV-*` identifier in
// `docs/spec.md`. Run `/spec-sync` to verify that no INV-* ID exists in one
// place but not the other.
//
// Style note: the *history-shape* invariants (INV-HIST-01, INV-HIST-03,
// INV-LIN-04) stay as macros because their per-call format strings interpolate
// caller-local fields (e.g. `op.client_id`). The *partition-shape* invariants
// (INV-LIN-03) are plain `pub(crate) fn`s — they take only `&[Vec<usize>]`
// (and for the events form, `&[Event<I, O>]`), gain nothing from macro
// expansion, and now share a single coverage scan.

use std::collections::HashMap;

use crate::types::{Event, EventKind};

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
//
// A `Model::partition` (or `partition_events`) implementation must produce
// index sets that are
//
//   (a) disjoint   — no index appears in two partitions
//   (b) complete   — every index in [0, history.len()) appears in exactly one
//                    partition
//   (c) in-bounds  — every index is `< history.len()`
//
// The events form additionally requires
//
//   (d) call/return paired — for every event id, the Call and Return events
//                            land in the same partition
//
// Violations of (a)/(b)/(c) make the per-partition DFS skip work or index out
// of bounds; (d) violations show the per-partition DFS a malformed
// sub-history. All four are debug-only invariants — the production binary
// trusts the partitioner.
//
// Formal counterpart: `tla/Porcupine.qnt :: pCompositionality` verifies the
// algorithmic side (a successful linearization is a real sequential
// execution); the functions below verify the partitioner-output side.

/// Walk every partition once, enforcing (a), (b), (c). Returns a per-index
/// table mapping each entry to the partition that owns it; the events form
/// reuses that table to check pairing without re-hashing.
///
/// `Vec<Option<u32>>` over `HashMap<usize, usize>` because indices are a dense
/// `[0, n)` range — direct array indexing is cheaper than hashing and gives
/// (b) for free as a length / count comparison.
///
/// # INV-LIN-03
fn scan_partition_covering(partitions: &[Vec<usize>], n: usize) -> Vec<Option<u32>> {
    let mut idx_to_part: Vec<Option<u32>> = vec![None; n];
    let mut count: usize = 0;
    for (part_idx, partition) in partitions.iter().enumerate() {
        let part_id = u32::try_from(part_idx).expect("partition count fits in u32");
        for &idx in partition.iter() {
            // (c) in-bounds
            debug_assert!(
                idx < n,
                "INV-LIN-03: partition index {} is out of bounds (history length {})",
                idx,
                n
            );
            // (a) disjoint
            debug_assert!(
                idx_to_part[idx].is_none(),
                "INV-LIN-03: index {} appears in more than one partition",
                idx
            );
            idx_to_part[idx] = Some(part_id);
            count += 1;
        }
    }
    // (b) complete
    debug_assert!(
        count == n,
        "INV-LIN-03: partitions cover {} entries but history has {} — \
         {} entry/entries missing from all partitions",
        count,
        n,
        n.saturating_sub(count)
    );
    idx_to_part
}

/// Operations form: assert the partition output is disjoint, complete, and
/// in-bounds.
///
/// Compiles out (other than the early-return) in release builds because every
/// check inside `scan_partition_covering` is `debug_assert!`.
///
/// # INV-LIN-03
#[inline]
pub(crate) fn assert_partition_covers_ops(partitions: &[Vec<usize>], history_len: usize) {
    if !cfg!(debug_assertions) {
        return;
    }
    let _ = scan_partition_covering(partitions, history_len);
}

/// Events form: assert the partition output is disjoint, complete, in-bounds,
/// AND keeps every `(Call, Return)` pair together (d).
///
/// Single linear pass over `history`: each event looks up its partition in
/// the dense `idx_to_part` table built by `scan_partition_covering`, and a
/// per-id `HashMap` records the partition observed at the Call so the matching
/// Return can compare. The event-id space is sparse (`u64`), so a `HashMap`
/// is the right structure here — unlike the dense per-index table.
///
/// # INV-LIN-03
#[inline]
pub(crate) fn assert_partition_events_paired<I, O>(
    partitions: &[Vec<usize>],
    history: &[Event<I, O>],
) {
    if !cfg!(debug_assertions) {
        return;
    }
    let n = history.len();
    let idx_to_part = scan_partition_covering(partitions, n);

    // event id → partition recorded at the Call event. `with_capacity(n / 2)`
    // because a well-formed history has exactly n/2 distinct ids.
    let mut call_part: HashMap<u64, u32> = HashMap::with_capacity(n / 2);
    for (pos, ev) in history.iter().enumerate() {
        // `expect` is sound: scan_partition_covering already asserted (b)
        // — every position in [0, n) has Some(_).
        let part = idx_to_part[pos].expect("INV-LIN-03: covers guarantees Some");
        match ev.kind {
            EventKind::Call => {
                call_part.insert(ev.id, part);
            }
            EventKind::Return => {
                if let Some(&cpart) = call_part.get(&ev.id) {
                    debug_assert!(
                        cpart == part,
                        "INV-LIN-03: event id={} has Call in partition {} but Return in partition {} \
                         — call/return pairs must not be split across partitions",
                        ev.id, cpart, part
                    );
                }
                // Returns without a matching Call are an INV-HIST-01 violation
                // — caught by `assert_well_formed_events!` upstream, so we
                // intentionally do nothing here.
            }
        }
    }
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
pub(crate) use assert_well_formed;
pub(crate) use assert_well_formed_events;

// ---------------------------------------------------------------------------
// Unit tests for the INV-LIN-03 partition functions
// ---------------------------------------------------------------------------
//
// Each test pins one of the four invariants (a)–(d) and confirms that a
// debug build panics when it is violated. The previous macro form could not
// host these tests cleanly; converting to functions enables this coverage.
//
// Gated on `debug_assertions` because the functions are intentional no-ops
// in release builds — `#[should_panic]` would never fire there, and a
// "valid partition passes" assertion would be tautological.

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use crate::types::{Event, EventKind};

    fn call(id: u64) -> Event<i32, i32> {
        Event {
            client_id: id,
            kind: EventKind::Call,
            input: Some(0),
            output: None,
            id,
        }
    }

    fn ret(id: u64) -> Event<i32, i32> {
        Event {
            client_id: id,
            kind: EventKind::Return,
            input: None,
            output: Some(0),
            id,
        }
    }

    /// INV-LIN-03 (a) + (b) + (c): a clean partitioning passes.
    #[test]
    fn ops_valid_partition_passes() {
        let parts = vec![vec![0, 2], vec![1, 3]];
        assert_partition_covers_ops(&parts, 4);
    }

    /// INV-LIN-03 (a): an index appearing in two partitions must panic.
    #[test]
    #[should_panic(expected = "INV-LIN-03")]
    fn ops_overlapping_partitions_panic() {
        let parts = vec![vec![0, 1], vec![1, 2]];
        assert_partition_covers_ops(&parts, 3);
    }

    /// INV-LIN-03 (b): an index never appearing in any partition must panic.
    #[test]
    #[should_panic(expected = "INV-LIN-03")]
    fn ops_missing_index_panics() {
        let parts = vec![vec![0, 2]];
        assert_partition_covers_ops(&parts, 3); // index 1 missing
    }

    /// INV-LIN-03 (c): an index ≥ history length must panic.
    #[test]
    #[should_panic(expected = "INV-LIN-03")]
    fn ops_out_of_bounds_panics() {
        let parts = vec![vec![0, 1, 2, 5]];
        assert_partition_covers_ops(&parts, 3);
    }

    /// INV-LIN-03 events form: a clean partitioning that keeps Call/Return
    /// pairs together passes.
    #[test]
    fn events_valid_partition_passes() {
        // history layout: [C0, C1, R0, R1] with id 0 in part 0, id 1 in part 1
        let history = vec![call(0), call(1), ret(0), ret(1)];
        let parts = vec![vec![0, 2], vec![1, 3]];
        assert_partition_events_paired(&parts, &history);
    }

    /// INV-LIN-03 (d): splitting a (Call, Return) pair across partitions must
    /// panic.
    #[test]
    #[should_panic(expected = "INV-LIN-03")]
    fn events_split_pair_panics() {
        // id 0's Call is at index 0 (part 0) but its Return is at index 2 (part 1).
        let history = vec![call(0), call(1), ret(0), ret(1)];
        let parts = vec![vec![0, 1], vec![2, 3]];
        assert_partition_events_paired(&parts, &history);
    }

    /// INV-LIN-03 events form (b): missing event index must panic before the
    /// pairing check even starts.
    #[test]
    #[should_panic(expected = "INV-LIN-03")]
    fn events_missing_index_panics() {
        let history = vec![call(0), call(1), ret(0), ret(1)];
        let parts = vec![vec![0, 2], vec![1]]; // index 3 missing
        assert_partition_events_paired(&parts, &history);
    }
}
