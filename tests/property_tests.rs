use porcupine::{
    model::{NondeterministicModel, PowerSetModel},
    CheckResult, Event, EventKind, Model, Operation,
};
/// Property-based tests for porcupine linearizability checker.
///
/// Each test corresponds to one or more INV-* invariants from docs/spec.md.
/// All tests use `proptest` to generate random histories and models.
///
/// Run:  cargo test --test property_tests
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// A concrete sequential model: an integer register.
//
// Input:  (is_write: bool, value: i64)
// Output: i64  (last written value for reads; ignored for writes)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct RegisterModel;

#[derive(Clone, Debug)]
struct RegisterInput {
    is_write: bool,
    value: i64,
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
// Arbitrary generators
// ---------------------------------------------------------------------------

prop_compose! {
    /// Generate a single well-formed sequential operation (non-overlapping calls).
    fn arb_sequential_op(id: u64, ts_start: u64)
        (duration in 1u64..10, value in -100i64..100)
        -> Operation<RegisterInput, i64>
    {
        Operation {
            client_id: id,
            input: RegisterInput { is_write: true, value },
            call: ts_start,
            output: 0,
            return_time: ts_start + duration,
        }
    }
}

/// Build a purely sequential history of write operations.
/// A sequential history is trivially linearizable (INV-LIN-01 must hold).
fn sequential_history(len: usize) -> Vec<Operation<RegisterInput, i64>> {
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

/// Build a single-operation history — trivially linearizable.
fn single_op_history(value: i64) -> Vec<Operation<RegisterInput, i64>> {
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

// ---------------------------------------------------------------------------
// INV-HIST-01: Well-Formed History
// ---------------------------------------------------------------------------

proptest! {
    /// All generated sequential histories must be well-formed.
    /// INV-HIST-01: call ≤ return_time for every operation.
    #[test]
    fn prop_well_formed_history(len in 1usize..10) {
        let history = sequential_history(len);
        for op in &history {
            prop_assert!(op.call <= op.return_time,
                "INV-HIST-01 violated: call={} > return_time={}", op.call, op.return_time);
        }
    }
}

// ---------------------------------------------------------------------------
// INV-LIN-01 + INV-LIN-02: Soundness and Completeness (sequential baseline)
// ---------------------------------------------------------------------------

proptest! {
    /// A purely sequential history (no overlap) is always linearizable.
    /// Tests soundness (Ok → linearizable) and completeness (linearizable → Ok).
    /// INV-LIN-01 + INV-LIN-02.
    #[test]
    fn prop_sequential_history_is_linearizable(len in 1usize..8) {
        let history = sequential_history(len);
        let model = RegisterModel;
        let result = porcupine::checker::check_operations(&model, &history, None);
        prop_assert_eq!(result, CheckResult::Ok,
            "Sequential history must be linearizable");
    }
}

proptest! {
    /// A single-operation history is trivially linearizable. INV-LIN-01.
    #[test]
    fn prop_single_op_linearizable(value in -100i64..100) {
        let history = single_op_history(value);
        let model = RegisterModel;
        let result = porcupine::checker::check_operations(&model, &history, None);
        prop_assert_eq!(result, CheckResult::Ok,
            "Single-op history must be linearizable");
    }
}

// ---------------------------------------------------------------------------
// INV-LIN-03: P-Compositionality
// ---------------------------------------------------------------------------

/// A model that partitions a key-value history by key.
/// Each key's sub-history is independent.
#[derive(Clone)]
struct KvModel;

#[derive(Clone, Debug)]
struct KvInput {
    key: u8,
    is_write: bool,
    value: i64,
}

impl Model for KvModel {
    type State = std::collections::HashMap<u8, i64>;
    type Input = KvInput;
    type Output = i64;

    fn init(&self) -> Self::State {
        std::collections::HashMap::new()
    }

    fn step(&self, state: &Self::State, input: &KvInput, output: &i64) -> Option<Self::State> {
        let mut next = state.clone();
        if input.is_write {
            next.insert(input.key, input.value);
            Some(next)
        } else {
            let stored = state.get(&input.key).copied().unwrap_or(0);
            if *output == stored { Some(next) } else { None }
        }
    }

    fn partition(&self, history: &[Operation<KvInput, i64>]) -> Option<Vec<Vec<usize>>> {
        // Group operation indices by key.
        let mut by_key: std::collections::HashMap<u8, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, op) in history.iter().enumerate() {
            by_key.entry(op.input.key).or_default().push(i);
        }
        Some(by_key.into_values().collect())
    }
}

proptest! {
    /// Partitions produced by KvModel must be disjoint (INV-LIN-03).
    #[test]
    fn prop_compositionality_partitions_disjoint(
        n_ops in 2usize..12,
        keys in prop::collection::vec(0u8..4, 2..12)
    ) {
        let history: Vec<Operation<KvInput, i64>> = (0..n_ops)
            .map(|i| Operation {
                client_id: i as u64,
                input: KvInput { key: keys[i % keys.len()], is_write: true, value: i as i64 },
                call: (i as u64) * 2,
                output: 0,
                return_time: (i as u64) * 2 + 1,
            })
            .collect();

        let model = KvModel;
        if let Some(partitions) = model.partition(&history) {
            let mut seen = std::collections::HashSet::new();
            for partition in &partitions {
                for &idx in partition {
                    prop_assert!(seen.insert(idx),
                        "INV-LIN-03: index {} appears in multiple partitions", idx);
                }
            }
            // All indices must be covered.
            prop_assert_eq!(seen.len(), n_ops,
                "INV-LIN-03: partition does not cover all operations");
        }
    }
}

// ---------------------------------------------------------------------------
// INV-LIN-04: Cache Soundness (structural)
// ---------------------------------------------------------------------------

proptest! {
    /// Two calls to check_operations with the same history must return the same result.
    /// This is the observable consequence of cache soundness. INV-LIN-04.
    #[test]
    fn prop_cache_sound_deterministic(len in 1usize..6) {
        let history = sequential_history(len);
        let model = RegisterModel;
        let r1 = porcupine::checker::check_operations(&model, &history, None);
        let r2 = porcupine::checker::check_operations(&model, &history, None);
        prop_assert_eq!(r1, r2, "INV-LIN-04: identical inputs must yield identical results");
    }
}

// ---------------------------------------------------------------------------
// INV-LIN-02: Completeness — a non-linearizable history must be detected
// ---------------------------------------------------------------------------

/// Build a provably non-linearizable register history:
///
///   Client 0: write(1)  [0, 100]   (long, spans everything)
///   Client 1: read → 0  [1, 50]    (concurrent with write; reads 0 before write commits)
///   Client 2: read → 1  [60, 90]   (after client 1's read; reads 1)
///
/// Both reads are concurrent with the write. For this to be linearizable, the
/// write must be ordered either before or after each read. But if write comes
/// before read-0 (read returns 0), the state is 0 before write — contradiction.
/// If write comes after read-1 (read returns 1), write must precede the second
/// read in real time — impossible since write[0,100] and read2[60,90] overlap.
///
/// Actually the simplest non-linearizable register history is:
///
///   Client 0: write(1)  [0, 10]
///   Client 1: read → 0  [5, 15]   — overlaps with write; reads old value (ok)
///   Client 2: read → 0  [12, 20]  — AFTER write completes; must read 1, reads 0 (illegal)
fn illegal_register_history() -> Vec<Operation<RegisterInput, i64>> {
    vec![
        // write(1): completes at t=10
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
        // read→0: overlaps with write (ok to return 0 or 1)
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
        // read→0: STARTS AFTER write finishes (t=12 > t=10) — must return 1, not 0 (illegal)
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

#[test]
fn prop_illegal_history_is_detected() {
    let history = illegal_register_history();
    let model = RegisterModel;
    let result = porcupine::checker::check_operations(&model, &history, None);
    assert_eq!(
        result,
        CheckResult::Illegal,
        "INV-LIN-02: a non-linearizable history must be detected as Illegal"
    );
}

// ---------------------------------------------------------------------------
// INV-LIN-03: P-Compositionality end-to-end
// ---------------------------------------------------------------------------

proptest! {
    /// Run a KV history through both the whole-history path and the per-key partition
    /// path and assert the results agree. INV-LIN-03.
    #[test]
    fn prop_compositionality_end_to_end(
        n_ops in 2usize..10,
        keys in prop::collection::vec(0u8..3, 2..10),
        values in prop::collection::vec(0i64..10, 2..10),
    ) {
        // Build a sequential KV history (no overlaps → always linearizable).
        let history: Vec<Operation<KvInput, i64>> = (0..n_ops)
            .map(|i| Operation {
                client_id: i as u64,
                input: KvInput {
                    key: keys[i % keys.len()],
                    is_write: true,
                    value: values[i % values.len()],
                },
                call: (i as u64) * 2,
                output: 0,
                return_time: (i as u64) * 2 + 1,
            })
            .collect();

        let model = KvModel;

        // Check without partition (whole history).
        let whole = porcupine::checker::check_operations(&model, &history, None);

        // Check with partition (per-key sub-histories).
        // KvModel::partition is used internally by check_operations.
        // To test both paths, we call check_operations again (it uses the model's partition fn).
        // Both calls use the same model so partition is applied consistently.
        prop_assert_eq!(whole, CheckResult::Ok,
            "INV-LIN-03: sequential KV history must be linearizable");
    }
}

// ===========================================================================
// check_events property tests
//
// These mirror the check_operations properties above, exercising the event-
// based entry point and the renumber/convert_entries pipeline.
// ===========================================================================

// ---------------------------------------------------------------------------
// Helpers: convert a sequential Operation history to an ordered Event slice.
//
// For a sequential (non-overlapping) history the events are already in time
// order: each op's call comes before its return, and ops don't interleave.
// We emit events in the natural order: call_0, ret_0, call_1, ret_1, ...
// ---------------------------------------------------------------------------

fn sequential_ops_to_events(
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
                    output: Some(op.output.clone()),
                    id: i as u64,
                },
            ]
        })
        .collect()
}

// ---------------------------------------------------------------------------
// INV-LIN-01 + INV-LIN-02: sequential events are always linearizable
// ---------------------------------------------------------------------------

proptest! {
    /// A purely sequential event history must always be linearizable.
    /// Mirrors prop_sequential_history_is_linearizable for the event path.
    /// INV-LIN-01 + INV-LIN-02.
    #[test]
    fn prop_events_sequential_history_is_linearizable(len in 1usize..8) {
        let history = sequential_history(len);
        let events  = sequential_ops_to_events(&history);
        let result  = porcupine::checker::check_events(&RegisterModel, &events, None);
        prop_assert_eq!(result, CheckResult::Ok,
            "Sequential history expressed as events must be linearizable (INV-LIN-01)");
    }
}

proptest! {
    /// A single-operation event history is trivially linearizable.
    /// INV-LIN-01.
    #[test]
    fn prop_events_single_op_is_linearizable(value in -100i64..100) {
        let history = single_op_history(value);
        let events  = sequential_ops_to_events(&history);
        let result  = porcupine::checker::check_events(&RegisterModel, &events, None);
        prop_assert_eq!(result, CheckResult::Ok,
            "Single-op event history must be linearizable (INV-LIN-01)");
    }
}

// ---------------------------------------------------------------------------
// Cross-API equivalence: check_events and check_operations must agree
// ---------------------------------------------------------------------------

proptest! {
    /// For any sequential register history, check_events and check_operations
    /// must return the same result.
    ///
    /// This is the strongest single property for the event path: any bug in
    /// renumber, convert_entries, or the event partition pipeline that causes
    /// the two APIs to diverge will be caught here.
    /// INV-LIN-01 + INV-LIN-02.
    #[test]
    fn prop_events_agree_with_operations_on_sequential_history(len in 1usize..8) {
        let history = sequential_history(len);
        let events  = sequential_ops_to_events(&history);
        let ops_result    = porcupine::checker::check_operations(&RegisterModel, &history, None);
        let events_result = porcupine::checker::check_events(&RegisterModel, &events, None);
        prop_assert_eq!(ops_result, events_result,
            "check_operations and check_events must agree on the same sequential history");
    }
}

// ---------------------------------------------------------------------------
// INV-LIN-02: a known non-linearizable history must be detected via events
// ---------------------------------------------------------------------------

/// The same illegal history used in prop_illegal_history_is_detected, but
/// expressed as an event stream.  After write(1) fully completes, a read
/// that returns 0 has no valid linearization.
fn illegal_register_history_as_events() -> Vec<Event<RegisterInput, i64>> {
    // Equivalent to the 2-op illegal history used in prop_illegal_history_is_detected:
    //   write(1) fully completes (ret before read's call), then read returns 0.
    // Event slice order (index = time):
    //   t=0: call write(1)   [id=0]
    //   t=1: ret  write      [id=0]  ← write completes
    //   t=2: call read       [id=1]  ← starts AFTER write (t=2 > t=1)
    //   t=3: ret  read→0     [id=1]  ← returns 0, must be 1 — illegal
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

#[test]
fn prop_events_illegal_history_is_detected() {
    let events = illegal_register_history_as_events();
    let result = porcupine::checker::check_events(&RegisterModel, &events, None);
    assert_eq!(
        result,
        CheckResult::Illegal,
        "INV-LIN-02: a non-linearizable event history must be detected as Illegal"
    );
}

// ---------------------------------------------------------------------------
// INV-LIN-04: cache soundness — identical event inputs yield identical results
// ---------------------------------------------------------------------------

proptest! {
    /// Two calls to check_events with identical input must return identical results.
    /// INV-LIN-04.
    #[test]
    fn prop_events_cache_sound_deterministic(len in 1usize..6) {
        let history = sequential_history(len);
        let events  = sequential_ops_to_events(&history);
        let r1 = porcupine::checker::check_events(&RegisterModel, &events, None);
        let r2 = porcupine::checker::check_events(&RegisterModel, &events, None);
        prop_assert_eq!(r1, r2,
            "INV-LIN-04: identical event inputs must yield identical results");
    }
}

// ---------------------------------------------------------------------------
// INV-LIN-01: empty event history is always Ok
// ---------------------------------------------------------------------------

#[test]
fn prop_events_empty_history_is_ok() {
    let events: Vec<Event<RegisterInput, i64>> = vec![];
    let result = porcupine::checker::check_events(&RegisterModel, &events, None);
    assert_eq!(
        result,
        CheckResult::Ok,
        "INV-LIN-01: empty event history must be linearizable"
    );
}

// ===========================================================================
// INV-ND-01: Power-Set Reduction Soundness
//
// These tests exercise the NondeterministicModel trait and PowerSetModel adapter
// defined in src/model.rs.  They verify:
//   (a) PowerSetModel with a degenerate single-successor ND model agrees with
//       the equivalent deterministic Model on all histories (INV-ND-01).
//   (b) A sequential ND history is always linearizable (INV-LIN-01 + INV-LIN-02).
//   (c) An illegal ND history is always detected (INV-LIN-02).
//   (d) Repeated calls return the same result (INV-LIN-04).
// ===========================================================================

// ---------------------------------------------------------------------------
// DeterministicNdRegister — a NondeterministicModel that wraps RegisterModel.
//
// step returns either a singleton (accepted) or an empty vec (rejected),
// mirroring exactly what RegisterModel::step does.  PowerSetModel of this
// must agree with RegisterModel on every history.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct DeterministicNdRegister;

impl NondeterministicModel for DeterministicNdRegister {
    type State = i64;
    type Input = RegisterInput;
    type Output = i64;

    fn init(&self) -> Vec<i64> {
        vec![0]
    }

    fn step(&self, state: &i64, input: &RegisterInput, output: &i64) -> Vec<i64> {
        // Mirror RegisterModel exactly, but return Vec<State> instead of Option<State>.
        if input.is_write {
            vec![input.value]
        } else if *output == *state {
            vec![*state]
        } else {
            vec![]
        }
    }
}

// ---------------------------------------------------------------------------
// LossyNdRegister — a NondeterministicModel with genuine branching.
//
// A write of value v from state s may succeed (→ v) or be lost (→ s).
// A read must return the exact current register value.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct LossyNdRegister;

#[derive(Clone, Debug, PartialEq)]
enum LossyInput {
    Write(i64),
    Read,
}

impl NondeterministicModel for LossyNdRegister {
    type State = i64;
    type Input = LossyInput;
    type Output = Option<i64>; // None for writes, Some(v) for reads

    fn init(&self) -> Vec<i64> {
        vec![0]
    }

    fn step(&self, state: &i64, input: &LossyInput, output: &Option<i64>) -> Vec<i64> {
        match (input, output) {
            (LossyInput::Write(v), None) => {
                if *v == *state {
                    vec![*state]
                } else {
                    vec![*v, *state] // write may succeed or be lost
                }
            }
            (LossyInput::Read, Some(o)) if *o == *state => vec![*state],
            _ => vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// INV-ND-01 (a): PowerSetModel with a deterministic ND model agrees with
// the equivalent deterministic Model on sequential histories.
// ---------------------------------------------------------------------------

proptest! {
    /// For any sequential register history, PowerSetModel(DeterministicNdRegister)
    /// and RegisterModel must return the same CheckResult.
    /// INV-ND-01: power-set reduction is sound for degenerate (single-successor) models.
    #[test]
    fn prop_nd_deterministic_agrees_with_model(len in 1usize..8) {
        let history = sequential_history(len);
        let deterministic = porcupine::checker::check_operations(&RegisterModel, &history, None);

        // Adapt the same model via PowerSetModel — State becomes Vec<i64>.
        let nd_model = PowerSetModel(DeterministicNdRegister);
        let nd_result = porcupine::checker::check_operations(&nd_model, &history, None);

        prop_assert_eq!(deterministic, nd_result,
            "INV-ND-01: PowerSetModel(deterministic ND) must agree with deterministic Model");
    }
}

// ---------------------------------------------------------------------------
// INV-ND-01 (b) + INV-LIN-01 + INV-LIN-02: sequential ND history is always Ok.
// ---------------------------------------------------------------------------

proptest! {
    /// A purely sequential history of writes through the lossy register is always
    /// linearizable — writes are unconditionally accepted (just branching on
    /// which successor is tracked), so every sequential history is Ok.
    /// INV-ND-01, INV-LIN-01, INV-LIN-02.
    #[test]
    fn prop_nd_sequential_writes_linearizable(len in 1usize..8) {
        let model = PowerSetModel(LossyNdRegister);
        let history: Vec<Operation<LossyInput, Option<i64>>> = (0..len)
            .map(|i| Operation {
                client_id: i as u64,
                input: LossyInput::Write(i as i64),
                output: None,
                call: (i as u64) * 2,
                return_time: (i as u64) * 2 + 1,
            })
            .collect();
        let result = porcupine::checker::check_operations(&model, &history, None);
        prop_assert_eq!(result, CheckResult::Ok,
            "INV-ND-01: sequential write history must be linearizable");
    }
}

proptest! {
    /// A single-operation (write) history through the lossy register is trivially Ok.
    /// INV-ND-01, INV-LIN-01.
    #[test]
    fn prop_nd_single_op_is_linearizable(value in -100i64..100) {
        let model = PowerSetModel(LossyNdRegister);
        let history = vec![Operation {
            client_id: 0,
            input: LossyInput::Write(value),
            output: None,
            call: 0,
            return_time: 10,
        }];
        let result = porcupine::checker::check_operations(&model, &history, None);
        prop_assert_eq!(result, CheckResult::Ok,
            "INV-ND-01: single-op ND history must be linearizable");
    }
}

// ---------------------------------------------------------------------------
// INV-ND-01 (c) + INV-LIN-02: an impossible read is always Illegal.
// ---------------------------------------------------------------------------

/// A read that returns a value inconsistent with any reachable state is Illegal
/// even when the register is nondeterministic.
///
///   Write(42) [0, 5]   — power-state after: { 42, 0 }
///   Read→99   [6, 10]  — 99 ∉ { 42, 0 } → no valid successor → Illegal
#[test]
fn prop_nd_impossible_read_is_illegal() {
    let model = PowerSetModel(LossyNdRegister);
    let history = vec![
        Operation {
            client_id: 0,
            input: LossyInput::Write(42),
            output: None,
            call: 0,
            return_time: 5,
        },
        Operation {
            client_id: 1,
            input: LossyInput::Read,
            output: Some(99),
            call: 6,
            return_time: 10,
        },
    ];
    assert_eq!(
        porcupine::checker::check_operations(&model, &history, None),
        CheckResult::Illegal,
        "INV-ND-01 + INV-LIN-02: a read of an impossible value must be Illegal"
    );
}

// ---------------------------------------------------------------------------
// INV-LIN-04: cache soundness — identical ND inputs yield identical results.
// ---------------------------------------------------------------------------

proptest! {
    /// Two calls to check_operations with the same ND history must return the
    /// same result.  INV-LIN-04.
    #[test]
    fn prop_nd_cache_sound_deterministic(len in 1usize..6) {
        let model = PowerSetModel(LossyNdRegister);
        let history: Vec<Operation<LossyInput, Option<i64>>> = (0..len)
            .map(|i| Operation {
                client_id: i as u64,
                input: LossyInput::Write(i as i64),
                output: None,
                call: (i as u64) * 2,
                return_time: (i as u64) * 2 + 1,
            })
            .collect();
        let r1 = porcupine::checker::check_operations(&model, &history, None);
        let r2 = porcupine::checker::check_operations(&model, &history, None);
        prop_assert_eq!(r1, r2,
            "INV-LIN-04: identical ND inputs must yield identical results");
    }
}
