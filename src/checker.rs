// Core linearizability checking logic — to be implemented.
//
// Algorithm outline:
//  1. Assert INV-HIST-01 (well-formed) on entry.
//  2. If the model provides `partition`, split the history (INV-LIN-03) and
//     check each sub-history independently.
//  3. Convert history into a linked-list of call/return entry pairs.
//  4. Run DFS with backtracking:
//     a. Collect the frontier of minimal calls (INV-HIST-03).
//     b. For each candidate, apply the model step; on success push to stack.
//     c. Cache (linearized_bitset, state) to prune duplicate branches (INV-LIN-04).
//     d. Backtrack when no candidate succeeds.
//  5. Return Ok if a complete linearization is found, Illegal otherwise.

use crate::invariants::{
    assert_partition_independent, assert_well_formed,
};
use crate::model::Model;
use crate::types::{CheckResult, Event, Operation};

/// Check an operation-based history for linearizability.
///
/// Returns `Ok` (linearizable), `Illegal` (not linearizable), or `Unknown` (not yet implemented).
pub fn check_operations<M: Model>(model: &M, history: &[Operation<M::Input, M::Output>]) -> CheckResult {
    // INV-HIST-01
    assert_well_formed!(history);

    // INV-LIN-03: validate partitions are disjoint if provided
    if let Some(partitions) = model.partition(history) {
        assert_partition_independent!(partitions);
    }

    let _ = model;
    // TODO: implement DFS backtracking
    CheckResult::Unknown
}

/// Check an event-based history for linearizability.
pub fn check_events<M: Model>(model: &M, history: &[Event<M::Input, M::Output>]) -> CheckResult {
    if let Some(partitions) = model.partition_events(history) {
        assert_partition_independent!(partitions);
    }

    let _ = model;
    // TODO: implement — convert events to operations then call check_operations
    CheckResult::Unknown
}
