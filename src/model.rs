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

/// A nondeterministic sequential specification.
///
/// Like [`Model`], but `step` returns *all* valid successor states for a given
/// `(state, input, output)` triple.  An empty `Vec` means the transition is
/// invalid; multiple entries mean any of those successors is reachable.
///
/// Use [`PowerSetModel`] to wrap a `NondeterministicModel` into a regular
/// [`Model`] that can be passed to [`crate::checker::check_operations`] or
/// [`crate::checker::check_events`].
pub trait NondeterministicModel {
    /// The concrete (per-branch) state type.
    type State: Clone + PartialEq;
    /// The type of operation inputs.
    type Input: Clone;
    /// The type of operation outputs.
    type Output: Clone;

    /// Returns all possible initial states.
    fn init(&self) -> Vec<Self::State>;

    /// Returns all valid successor states for the given transition.
    ///
    /// An empty `Vec` means the transition is rejected.
    fn step(
        &self,
        state: &Self::State,
        input: &Self::Input,
        output: &Self::Output,
    ) -> Vec<Self::State>;

    /// Optionally partition an operation history into independent sub-histories.
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

/// Adapts a [`NondeterministicModel`] into a regular [`Model`] via the
/// power-set construction.
///
/// The adapted model's `State` is `Vec<M::State>` — the set of all concrete
/// states the system could currently be in.  The `step` function fans out over
/// every state in the current power-state, collects all successors, deduplicates
/// them (using `PartialEq`), and returns `Some(deduplicated_set)` when at least
/// one successor exists, or `None` when none do.
///
/// # Example
///
/// ```rust
/// use porcupine::model::{NondeterministicModel, PowerSetModel};
/// use porcupine::checker::check_operations;
/// use porcupine::types::{CheckResult, Operation};
///
/// struct MyNdModel;
///
/// impl NondeterministicModel for MyNdModel {
///     type State = u32;
///     type Input  = ();
///     type Output = ();
///
///     fn init(&self) -> Vec<u32> { vec![0] }
///
///     fn step(&self, state: &u32, _input: &(), _output: &()) -> Vec<u32> {
///         vec![state + 1, state + 2]   // two valid successors
///     }
/// }
///
/// let model = PowerSetModel(MyNdModel);
/// let history: Vec<Operation<(), ()>> = vec![];
/// let result = check_operations(&model, &history, None);
/// assert_eq!(result, CheckResult::Ok);
/// ```
pub struct PowerSetModel<M>(pub M);

impl<M> Model for PowerSetModel<M>
where
    M: NondeterministicModel,
    M::State: Clone + PartialEq,
    M::Input: Clone,
    M::Output: Clone,
{
    /// The power-state: the set of all concrete states reachable so far.
    type State = Vec<M::State>;
    type Input = M::Input;
    type Output = M::Output;

    fn init(&self) -> Self::State {
        deduplicate(self.0.init())
    }

    fn step(
        &self,
        state: &Self::State,
        input: &Self::Input,
        output: &Self::Output,
    ) -> Option<Self::State> {
        let mut successors: Vec<M::State> = Vec::new();
        for s in state {
            successors.extend(self.0.step(s, input, output));
        }
        let deduped = deduplicate(successors);
        if deduped.is_empty() {
            None
        } else {
            Some(deduped)
        }
    }

    fn partition(
        &self,
        history: &[Operation<Self::Input, Self::Output>],
    ) -> Option<Vec<Vec<usize>>> {
        self.0.partition(history)
    }

    fn partition_events(
        &self,
        history: &[Event<Self::Input, Self::Output>],
    ) -> Option<Vec<Vec<usize>>> {
        self.0.partition_events(history)
    }
}

/// Remove duplicate states from `states`, preserving first-occurrence order.
fn deduplicate<S: PartialEq>(states: Vec<S>) -> Vec<S> {
    let mut out: Vec<S> = Vec::with_capacity(states.len());
    for s in states {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}
