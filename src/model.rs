use crate::types::{Event, Operation};

/// A sequential specification that a concurrent history is checked against.
///
/// Implement this trait to define the state machine your system should conform to.
pub trait Model {
    /// The type representing the state of the model.
    type State: Clone + PartialEq;
    /// The type of operation inputs.
    type Input: Clone;
    /// The type of operation outputs.
    type Output: Clone;

    /// Returns the initial state of the model.
    fn init(&self) -> Self::State;

    /// Attempts to apply `input`/`output` to `state`.
    ///
    /// Returns `Some(next_state)` if the transition is valid, `None` otherwise.
    fn step(
        &self,
        state: &Self::State,
        input: &Self::Input,
        output: &Self::Output,
    ) -> Option<Self::State>;

    /// Optionally partition a history into independent sub-histories.
    ///
    /// Returning `None` (the default) disables partitioning; the whole history
    /// is checked as one unit.  Partitioning can yield dramatic speedups for
    /// models like key-value stores where operations on different keys are
    /// independent.
    fn partition(
        &self,
        _history: &[Operation<Self::Input, Self::Output>],
    ) -> Option<Vec<Vec<usize>>> {
        None
    }

    /// Optionally partition an event-based history into independent sub-histories.
    fn partition_events(
        &self,
        _history: &[Event<Self::Input, Self::Output>],
    ) -> Option<Vec<Vec<usize>>> {
        None
    }
}
