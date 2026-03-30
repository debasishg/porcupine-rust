// Core linearizability checking logic — to be implemented.
//
// Algorithm outline:
//  1. Convert history into a doubly-linked list of (call, return) entry pairs.
//  2. Run depth-first search with backtracking:
//     - At each step, collect all "minimal" calls (those whose call event has
//       no preceding unmatched call).
//     - Try to linearize each minimal call against the current model state.
//     - Cache (linearized_bitset, state) pairs to prune duplicate branches.
//  3. If the model provides a `partition` function, split the history and check
//     each partition independently (P-compositionality).

use crate::model::Model;
use crate::types::{CheckResult, Event, Operation};

/// Check an operation-based history for linearizability.
pub fn check_operations<M: Model>(model: &M, history: &[Operation<M::Input, M::Output>]) -> CheckResult {
    let _ = (model, history);
    // TODO: implement
    CheckResult::Unknown
}

/// Check an event-based history for linearizability.
pub fn check_events<M: Model>(model: &M, history: &[Event<M::Input, M::Output>]) -> CheckResult {
    let _ = (model, history);
    // TODO: implement
    CheckResult::Unknown
}
