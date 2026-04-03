/// The outcome of a linearizability check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum CheckResult {
    /// The history is linearizable.
    Ok,
    /// The history is not linearizable.
    Illegal,
    /// The check timed out before a definitive answer was reached.
    Unknown,
}

/// A completed concurrent operation with call/return timestamps.
#[derive(Debug, Clone)]
pub struct Operation<I, O> {
    /// Identifier for the client that issued this operation.
    pub client_id: u64,
    /// Input to the operation.
    pub input: I,
    /// Timestamp (in nanoseconds) when the operation was invoked.
    pub call: u64,
    /// Output returned by the operation.
    pub output: O,
    /// Timestamp (in nanoseconds) when the operation returned.
    pub return_time: u64,
}

/// Whether an event is a call or a return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Call,
    Return,
}

/// A single call or return event in a history.
#[derive(Debug, Clone)]
pub struct Event<I, O> {
    /// Identifier for the client that issued this event.
    pub client_id: u64,
    /// Whether this is a call or a return.
    pub kind: EventKind,
    /// For a call event: the input value. For a return event: `None`.
    pub input: Option<I>,
    /// For a return event: the output value. For a call event: `None`.
    pub output: Option<O>,
    /// Unique identifier linking a call event to its matching return event.
    pub id: u64,
}

/// Diagnostic information about a (partial) linearization, used for visualization.
///
/// **Not yet populated.** The Go original populates this via `CheckOperationsVerbose` /
/// `CheckEventsVerbose`; a Rust equivalent is planned but not yet implemented.
#[derive(Debug, Clone, Default)]
pub struct LinearizationInfo {
    /// For each partition, the sequence of operation indices in linearization order.
    pub partitions: Vec<Vec<Vec<usize>>>,
}
