// Runtime invariant assertions for porcupine-rust.
//
// Every macro here corresponds to an `INV-*` identifier in `docs/spec.md`.
// Run `/spec-sync` to verify that no INV-* ID exists in one place but not the other.
//
// Macros exported below but not yet called at all DFS call-sites (implementation
// is still a stub) produce spurious unused warnings — suppressed here.
#![allow(unused_macros, unused_imports)]

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
/// # INV-LIN-03
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
// INV-LIN-04: Cache Soundness
// ---------------------------------------------------------------------------

/// Assert that a cache hit is only used when the stored state equals the current
/// model state (same bitset key + same state → same result).
///
/// # INV-LIN-04
macro_rules! assert_cache_sound {
    ($cached_state:expr, $current_state:expr) => {
        debug_assert!(
            $cached_state == $current_state,
            "INV-LIN-04: cache hit on matching bitset but states differ — cache key collision"
        );
    };
}

pub(crate) use assert_cache_sound;
pub(crate) use assert_minimal_call;
pub(crate) use assert_partition_independent;
pub(crate) use assert_well_formed;
