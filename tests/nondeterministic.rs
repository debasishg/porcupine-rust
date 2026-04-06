/// Integration tests for `NondeterministicModel` + `PowerSetModel`.
///
/// Each test uses a small hand-crafted nondeterministic model and verifies that
/// `check_operations` / `check_events` returns the expected `CheckResult` when
/// the model is wrapped in `PowerSetModel`.
use porcupine::{
    checker::{check_events, check_operations},
    model::{NondeterministicModel, PowerSetModel},
    types::{CheckResult, Event, EventKind, Operation},
};

// ---------------------------------------------------------------------------
// Model 1 — BranchingCounter
//
// A counter that can increment by either 1 or 2 on every operation.
// Input:  () (no client-supplied value)
// Output: u32 — the value the client observes after the operation.
//
// `step`: for each current counter value `v`, the operation is valid if the
// observed output equals `v + 1` or `v + 2`.  Both are valid successors.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Debug)]
struct BranchingCounter;

impl NondeterministicModel for BranchingCounter {
    type State = u32;
    type Input = ();
    type Output = u32;

    fn init(&self) -> Vec<u32> {
        vec![0]
    }

    fn step(&self, state: &u32, _input: &(), output: &u32) -> Vec<u32> {
        let mut successors = Vec::new();
        if *output == state + 1 {
            successors.push(state + 1);
        }
        if *output == state + 2 {
            successors.push(state + 2);
        }
        successors
    }
}

fn op(output: u32, call: u64, return_time: u64) -> Operation<(), u32> {
    Operation {
        client_id: 0,
        input: (),
        call,
        output,
        return_time,
    }
}

// A single operation that observes 1 (= 0+1): linearizable.
#[test]
fn branching_counter_single_op_ok() {
    let model = PowerSetModel(BranchingCounter);
    let history = vec![op(1, 0, 10)];
    assert_eq!(check_operations(&model, &history, None), CheckResult::Ok);
}

// A single operation that observes 2 (= 0+2): also linearizable (skip branch).
#[test]
fn branching_counter_single_op_skip_ok() {
    let model = PowerSetModel(BranchingCounter);
    let history = vec![op(2, 0, 10)];
    assert_eq!(check_operations(&model, &history, None), CheckResult::Ok);
}

// An observation of 3 from state 0 is impossible (neither 0+1 nor 0+2 = 3).
#[test]
fn branching_counter_single_op_illegal() {
    let model = PowerSetModel(BranchingCounter);
    let history = vec![op(3, 0, 10)];
    assert_eq!(
        check_operations(&model, &history, None),
        CheckResult::Illegal
    );
}

// Sequential history: 1 then 2.
// After observing 1 the counter is at 1; observing 2 next means 1+1=2 ✓.
#[test]
fn branching_counter_sequential_ok() {
    let model = PowerSetModel(BranchingCounter);
    let history = vec![op(1, 0, 5), op(2, 6, 10)];
    assert_eq!(check_operations(&model, &history, None), CheckResult::Ok);
}

// Sequential history: 1 then 4.
// After state 1 the only valid observations are 2 or 3 — not 4.
#[test]
fn branching_counter_sequential_illegal() {
    let model = PowerSetModel(BranchingCounter);
    let history = vec![op(1, 0, 5), op(4, 6, 10)];
    assert_eq!(
        check_operations(&model, &history, None),
        CheckResult::Illegal
    );
}

// Overlapping operations:
//   op A: call=0, return=10, out=1
//   op B: call=5, return=15, out=2
// Ordering A→B: state 0→1 (out=1 ✓), state 1→2 (1+1=2 ✓) — valid.
#[test]
fn branching_counter_concurrent_ok() {
    let model = PowerSetModel(BranchingCounter);
    let history = vec![
        Operation {
            client_id: 0,
            input: (),
            output: 1u32,
            call: 0,
            return_time: 10,
        },
        Operation {
            client_id: 1,
            input: (),
            output: 2u32,
            call: 5,
            return_time: 15,
        },
    ];
    assert_eq!(check_operations(&model, &history, None), CheckResult::Ok);
}

// ---------------------------------------------------------------------------
// Model 2 — NdRegister (nondeterministic / lossy register)
//
// A register where a write can either succeed (updating the stored value)
// or be silently lost (leaving the old value).  A read must return the
// current stored value.
//
// Input:  RegOp::Write(v) | RegOp::Read
// Output: Option<u32>  — None for writes, Some(observed) for reads.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Debug)]
enum RegOp {
    Write(u32),
    Read,
}

#[derive(Clone, PartialEq, Debug)]
struct NdRegister;

impl NondeterministicModel for NdRegister {
    type State = u32;
    type Input = RegOp;
    type Output = Option<u32>;

    fn init(&self) -> Vec<u32> {
        vec![0]
    }

    fn step(&self, state: &u32, input: &RegOp, output: &Option<u32>) -> Vec<u32> {
        match (input, output) {
            (RegOp::Write(v), None) => {
                // Lossy write: the register may update to *v* or stay at *state*.
                if *v == *state {
                    vec![*state]
                } else {
                    vec![*v, *state]
                }
            }
            (RegOp::Read, Some(observed)) if *observed == *state => vec![*state],
            _ => vec![],
        }
    }
}

fn reg_write(v: u32, call: u64, return_time: u64, client_id: u64) -> Operation<RegOp, Option<u32>> {
    Operation {
        client_id,
        input: RegOp::Write(v),
        output: None,
        call,
        return_time,
    }
}

fn reg_read(v: u32, call: u64, return_time: u64, client_id: u64) -> Operation<RegOp, Option<u32>> {
    Operation {
        client_id,
        input: RegOp::Read,
        output: Some(v),
        call,
        return_time,
    }
}

// Write 42, read 0 — valid because the write may have been lost.
#[test]
fn nd_register_lossy_write_read_old_ok() {
    let model = PowerSetModel(NdRegister);
    let history = vec![reg_write(42, 0, 5, 0), reg_read(0, 6, 10, 1)];
    assert_eq!(check_operations(&model, &history, None), CheckResult::Ok);
}

// Write 42, read 42 — valid because the write may have succeeded.
#[test]
fn nd_register_lossy_write_read_new_ok() {
    let model = PowerSetModel(NdRegister);
    let history = vec![reg_write(42, 0, 5, 0), reg_read(42, 6, 10, 1)];
    assert_eq!(check_operations(&model, &history, None), CheckResult::Ok);
}

// Write 42, read 99 — invalid (neither 0 nor 42 can produce 99).
#[test]
fn nd_register_lossy_write_read_illegal() {
    let model = PowerSetModel(NdRegister);
    let history = vec![reg_write(42, 0, 5, 0), reg_read(99, 6, 10, 1)];
    assert_eq!(
        check_operations(&model, &history, None),
        CheckResult::Illegal
    );
}

// ---------------------------------------------------------------------------
// check_events API — BranchingCounter model via Event history
// ---------------------------------------------------------------------------

fn call_evt(id: u64, client_id: u64) -> Event<(), u32> {
    Event {
        client_id,
        kind: EventKind::Call,
        input: Some(()),
        output: None,
        id,
    }
}

fn ret_evt(id: u64, client_id: u64, output: u32) -> Event<(), u32> {
    Event {
        client_id,
        kind: EventKind::Return,
        input: None,
        output: Some(output),
        id,
    }
}

#[test]
fn branching_counter_events_single_ok() {
    let model = PowerSetModel(BranchingCounter);
    let history = vec![call_evt(0, 0), ret_evt(0, 0, 1)];
    assert_eq!(check_events(&model, &history, None), CheckResult::Ok);
}

#[test]
fn branching_counter_events_single_illegal() {
    let model = PowerSetModel(BranchingCounter);
    let history = vec![call_evt(0, 0), ret_evt(0, 0, 5)];
    assert_eq!(
        check_events(&model, &history, None),
        CheckResult::Illegal
    );
}

// op 0: out=2 (0→2, skip branch)
// op 1: out=3 (2→3, increment branch)
#[test]
fn branching_counter_events_sequential_ok() {
    let model = PowerSetModel(BranchingCounter);
    let history = vec![
        call_evt(0, 0),
        ret_evt(0, 0, 2),
        call_evt(1, 0),
        ret_evt(1, 0, 3),
    ];
    assert_eq!(check_events(&model, &history, None), CheckResult::Ok);
}
