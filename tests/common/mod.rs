//! Shared test fixtures: sequential models and history builders used by both
//! the proptest and Hegel property suites.
//!
//! ## Layout note
//!
//! Cargo compiles every `tests/*.rs` file as its own integration-test crate.
//! To share code between them without producing an empty extra binary, this
//! file lives at `tests/common/mod.rs` (a directory module). Each test file
//! declares `mod common;` to pull it in.
//!
//! Because each test crate uses only a subset of the items below, we silence
//! `dead_code` at the module level rather than annotating every item.

#![allow(dead_code)]

use std::collections::HashMap;

use porcupine::{Event, EventKind, Model, Operation, model::NondeterministicModel};

// ---------------------------------------------------------------------------
// Register model — single integer cell
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct RegisterModel;

#[derive(Clone, Debug)]
pub struct RegisterInput {
    pub is_write: bool,
    pub value: i64,
}

impl Model for RegisterModel {
    type State = i64;
    type Input = RegisterInput;
    type Output = i64;

    fn init(&self) -> i64 {
        0
    }

    fn step(&self, state: &i64, input: &RegisterInput, output: &i64) -> Option<i64> {
        if input.is_write {
            Some(input.value)
        } else if *output == *state {
            Some(*state)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Key-value model — partitioned by key (used for INV-LIN-03 tests)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct KvModel;

#[derive(Clone, Debug)]
pub struct KvInput {
    pub key: u8,
    pub is_write: bool,
    pub value: i64,
}

impl Model for KvModel {
    type State = HashMap<u8, i64>;
    type Input = KvInput;
    type Output = i64;

    fn init(&self) -> Self::State {
        HashMap::new()
    }

    fn step(&self, state: &Self::State, input: &KvInput, output: &i64) -> Option<Self::State> {
        let mut next = state.clone();
        if input.is_write {
            next.insert(input.key, input.value);
            Some(next)
        } else {
            let stored = state.get(&input.key).copied().unwrap_or(0);
            if *output == stored {
                Some(next)
            } else {
                None
            }
        }
    }

    fn partition(&self, history: &[Operation<KvInput, i64>]) -> Option<Vec<Vec<usize>>> {
        let mut by_key: HashMap<u8, Vec<usize>> = HashMap::new();
        for (i, op) in history.iter().enumerate() {
            by_key.entry(op.input.key).or_default().push(i);
        }
        Some(by_key.into_values().collect())
    }
}

// ---------------------------------------------------------------------------
// NondeterministicModel fixtures (INV-ND-01 tests)
// ---------------------------------------------------------------------------

/// Wraps `RegisterModel` as a `NondeterministicModel` with single-successor
/// `step` (or empty for rejection). Used to verify that `PowerSetModel` of a
/// degenerate ND model agrees with the equivalent deterministic `Model`.
#[derive(Clone)]
pub struct DeterministicNdRegister;

impl NondeterministicModel for DeterministicNdRegister {
    type State = i64;
    type Input = RegisterInput;
    type Output = i64;

    fn init(&self) -> Vec<i64> {
        vec![0]
    }

    fn step(&self, state: &i64, input: &RegisterInput, output: &i64) -> Vec<i64> {
        if input.is_write {
            vec![input.value]
        } else if *output == *state {
            vec![*state]
        } else {
            vec![]
        }
    }
}

/// A genuinely branching ND register: a write of `v` from state `s` may
/// succeed (`→ v`) or be lost (`→ s`). Reads must return the exact current
/// register value.
#[derive(Clone)]
pub struct LossyNdRegister;

#[derive(Clone, Debug, PartialEq)]
pub enum LossyInput {
    Write(i64),
    Read,
}

impl NondeterministicModel for LossyNdRegister {
    type State = i64;
    type Input = LossyInput;
    type Output = Option<i64>;

    fn init(&self) -> Vec<i64> {
        vec![0]
    }

    fn step(&self, state: &i64, input: &LossyInput, output: &Option<i64>) -> Vec<i64> {
        match (input, output) {
            (LossyInput::Write(v), None) => {
                if *v == *state {
                    vec![*state]
                } else {
                    vec![*v, *state]
                }
            }
            (LossyInput::Read, Some(o)) if *o == *state => vec![*state],
            _ => vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// Partition wrappers (used by §3 P-compositionality equivalence tests)
// ---------------------------------------------------------------------------

/// `KvModel` step semantics with `partition` disabled — `check_operations`
/// runs the whole history as a single partition.
#[derive(Clone)]
pub struct KvNoPartition;

impl Model for KvNoPartition {
    type State = HashMap<u8, i64>;
    type Input = KvInput;
    type Output = i64;
    fn init(&self) -> Self::State {
        HashMap::new()
    }
    fn step(&self, state: &Self::State, input: &KvInput, output: &i64) -> Option<Self::State> {
        KvModel.step(state, input, output)
    }
}

/// `KvModel` step semantics with `partition` returning the same partitions
/// in *reversed* order. Used by the partition-order-invariance test.
#[derive(Clone)]
pub struct KvModelReversedPartition;

impl Model for KvModelReversedPartition {
    type State = HashMap<u8, i64>;
    type Input = KvInput;
    type Output = i64;
    fn init(&self) -> Self::State {
        HashMap::new()
    }
    fn step(&self, state: &Self::State, input: &KvInput, output: &i64) -> Option<Self::State> {
        KvModel.step(state, input, output)
    }
    fn partition(&self, history: &[Operation<KvInput, i64>]) -> Option<Vec<Vec<usize>>> {
        let mut p = KvModel.partition(history)?;
        p.reverse();
        Some(p)
    }
}

// ---------------------------------------------------------------------------
// Degenerate ND fixtures (used by §4 always-reject / always-stutter tests)
// ---------------------------------------------------------------------------

/// Nondeterministic model whose `step` always returns `vec![]` — every
/// transition is rejected. Wrapped in `PowerSetModel`, every non-empty
/// history must be `Illegal`.
#[derive(Clone)]
pub struct AlwaysRejectNd;

impl NondeterministicModel for AlwaysRejectNd {
    type State = ();
    type Input = ();
    type Output = ();
    fn init(&self) -> Vec<()> {
        vec![()]
    }
    fn step(&self, _: &(), _: &(), _: &()) -> Vec<()> {
        vec![]
    }
}

/// Nondeterministic model whose `step` always stutters (returns the same
/// state). Wrapped in `PowerSetModel`, every history must be `Ok`.
#[derive(Clone)]
pub struct AlwaysStutterNd;

impl NondeterministicModel for AlwaysStutterNd {
    type State = ();
    type Input = ();
    type Output = ();
    fn init(&self) -> Vec<()> {
        vec![()]
    }
    fn step(&self, _: &(), _: &(), _: &()) -> Vec<()> {
        vec![()]
    }
}

// ---------------------------------------------------------------------------
// History builders
// ---------------------------------------------------------------------------

/// Build a purely sequential history of `len` writes (write `i` from client
/// `i`). Sequential histories never overlap and are trivially linearizable.
pub fn sequential_history(len: usize) -> Vec<Operation<RegisterInput, i64>> {
    let mut ts = 0u64;
    (0..len)
        .map(|i| {
            let call = ts;
            let return_time = ts + 5;
            ts = return_time + 1;
            Operation {
                client_id: i as u64,
                input: RegisterInput {
                    is_write: true,
                    value: i as i64,
                },
                call,
                output: 0,
                return_time,
            }
        })
        .collect()
}

/// Build a single-write history with the given value.
pub fn single_op_history(value: i64) -> Vec<Operation<RegisterInput, i64>> {
    vec![Operation {
        client_id: 0,
        input: RegisterInput {
            is_write: true,
            value,
        },
        call: 0,
        output: 0,
        return_time: 10,
    }]
}

/// A provably non-linearizable register history (used by INV-LIN-02 tests):
///
/// ```text
/// Client 0: write(1)  [0, 10]
/// Client 1: read → 0  [5, 15]    — overlaps the write; either ordering is ok
/// Client 2: read → 0  [12, 20]   — starts AFTER write completes, so must
///                                   return 1 — returning 0 is illegal
/// ```
pub fn illegal_register_history() -> Vec<Operation<RegisterInput, i64>> {
    vec![
        Operation {
            client_id: 0,
            input: RegisterInput {
                is_write: true,
                value: 1,
            },
            call: 0,
            output: 0,
            return_time: 10,
        },
        Operation {
            client_id: 1,
            input: RegisterInput {
                is_write: false,
                value: 0,
            },
            call: 5,
            output: 0,
            return_time: 15,
        },
        Operation {
            client_id: 2,
            input: RegisterInput {
                is_write: false,
                value: 0,
            },
            call: 12,
            output: 0,
            return_time: 20,
        },
    ]
}

/// Convert a sequential operation history to an ordered event slice (call,
/// then return, for each operation in turn).
pub fn sequential_ops_to_events(
    ops: &[Operation<RegisterInput, i64>],
) -> Vec<Event<RegisterInput, i64>> {
    ops.iter()
        .enumerate()
        .flat_map(|(i, op)| {
            [
                Event {
                    client_id: op.client_id,
                    kind: EventKind::Call,
                    input: Some(op.input.clone()),
                    output: None,
                    id: i as u64,
                },
                Event {
                    client_id: op.client_id,
                    kind: EventKind::Return,
                    input: None,
                    output: Some(op.output),
                    id: i as u64,
                },
            ]
        })
        .collect()
}

/// Build a 2-op (write, read) history where the write occupies
/// `[0, write_dur]` and the read occupies
/// `[read_call, read_call + read_dur]`. Caller must ensure
/// `read_call < write_dur` for the windows to overlap.
pub fn build_overlap_write_read(
    write_value: i64,
    write_dur: u64,
    read_call: u64,
    read_dur: u64,
    read_output: i64,
) -> Vec<Operation<RegisterInput, i64>> {
    vec![
        Operation {
            client_id: 0,
            input: RegisterInput {
                is_write: true,
                value: write_value,
            },
            call: 0,
            output: 0,
            return_time: write_dur,
        },
        Operation {
            client_id: 1,
            input: RegisterInput {
                is_write: false,
                value: 0,
            },
            call: read_call,
            output: read_output,
            return_time: read_call + read_dur,
        },
    ]
}

/// Build a 3-op (write1, write2, late-read) history where the two writes
/// overlap (caller ensures `w2_call < t1`) and the read is strictly after
/// both writes return.
pub fn build_two_writers_late_reader(
    v1: i64,
    v2: i64,
    t1: u64,
    w2_call: u64,
    w2_dur: u64,
    r_dur: u64,
    read_output: i64,
) -> Vec<Operation<RegisterInput, i64>> {
    let w2_return = w2_call + w2_dur;
    let r_call = std::cmp::max(t1, w2_return) + 1;
    vec![
        Operation {
            client_id: 0,
            input: RegisterInput {
                is_write: true,
                value: v1,
            },
            call: 0,
            output: 0,
            return_time: t1,
        },
        Operation {
            client_id: 1,
            input: RegisterInput {
                is_write: true,
                value: v2,
            },
            call: w2_call,
            output: 0,
            return_time: w2_return,
        },
        Operation {
            client_id: 2,
            input: RegisterInput {
                is_write: false,
                value: 0,
            },
            call: r_call,
            output: read_output,
            return_time: r_call + r_dur,
        },
    ]
}

/// Convert any operation history to an event slice ordered by time, with
/// calls preceding returns at equal timestamps. Suitable for *concurrent*
/// (overlapping) histories — unlike [`sequential_ops_to_events`] which
/// emits events in slice order and is only correct when ops don't overlap.
///
/// Mirrors the tiebreak in `checker::make_entries`: at equal timestamps a
/// `Call` always sorts before a `Return`.
pub fn ops_to_events_sorted_by_time<I: Clone, O: Clone>(
    ops: &[Operation<I, O>],
) -> Vec<Event<I, O>> {
    let mut entries: Vec<(u64, bool, usize)> = Vec::with_capacity(ops.len() * 2);
    for (i, op) in ops.iter().enumerate() {
        entries.push((op.call, true, i));        // true = Call
        entries.push((op.return_time, false, i)); // false = Return
    }
    entries.sort_by(|a, b| {
        a.0.cmp(&b.0).then_with(|| match (a.1, b.1) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        })
    });
    entries
        .into_iter()
        .map(|(_, is_call, i)| {
            let op = &ops[i];
            if is_call {
                Event {
                    client_id: op.client_id,
                    kind: EventKind::Call,
                    input: Some(op.input.clone()),
                    output: None,
                    id: i as u64,
                }
            } else {
                Event {
                    client_id: op.client_id,
                    kind: EventKind::Return,
                    input: None,
                    output: Some(op.output.clone()),
                    id: i as u64,
                }
            }
        })
        .collect()
}

/// A 2-op illegal event history (write completes, subsequent read returns 0).
pub fn illegal_register_history_as_events() -> Vec<Event<RegisterInput, i64>> {
    vec![
        Event {
            client_id: 0,
            kind: EventKind::Call,
            input: Some(RegisterInput {
                is_write: true,
                value: 1,
            }),
            output: None,
            id: 0,
        },
        Event {
            client_id: 0,
            kind: EventKind::Return,
            input: None,
            output: Some(0),
            id: 0,
        },
        Event {
            client_id: 1,
            kind: EventKind::Call,
            input: Some(RegisterInput {
                is_write: false,
                value: 0,
            }),
            output: None,
            id: 1,
        },
        Event {
            client_id: 1,
            kind: EventKind::Return,
            input: None,
            output: Some(0),
            id: 1,
        },
    ]
}
