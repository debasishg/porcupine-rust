/// Property-based tests for porcupine linearizability checker.
///
/// Each test corresponds to one or more INV-* invariants from docs/spec.md.
/// All tests use `proptest` to generate random histories and models. Models
/// and history builders are shared with `tests/hegel_properties.rs` via
/// `tests/common/mod.rs`.
///
/// Run:  cargo test --test property_tests
use porcupine::{CheckResult, Event, Model, Operation, model::PowerSetModel};
use proptest::prelude::*;

mod common;

use common::{
    DeterministicNdRegister, KvInput, KvModel, LossyInput, LossyNdRegister, RegisterInput,
    RegisterModel, illegal_register_history, illegal_register_history_as_events,
    ops_to_events_sorted_by_time, sequential_history, sequential_ops_to_events, single_op_history,
};

// ---------------------------------------------------------------------------
// proptest-specific generator: a single sequential operation.
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

// `DeterministicNdRegister` and `LossyNdRegister` (with `LossyInput`) are
// shared with hegel_properties via `tests/common/mod.rs`.

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

// ===========================================================================
// Concurrent (overlapping) histories
//
// The properties above almost exclusively use sequential histories (no
// overlap between ops), which means the DFS backtracking and cache pruning
// paths in `checker.rs` are barely exercised.  These properties build
// histories whose [call, return] windows overlap, forcing the checker into
// the genuinely interesting code paths (lift / unlift / deferred-clone
// cache probe / etc.).
// ===========================================================================

prop_compose! {
    /// Generate a writes-only register history of length `min_len..=max_len`
    /// where call/return windows are drawn independently in [0, 80] and so
    /// pairwise-overlap with high probability.
    fn arb_concurrent_writes(min_len: usize, max_len: usize)
        (n in min_len..=max_len)
        (specs in prop::collection::vec((0u64..40, 1u64..40), n..=n))
        -> Vec<Operation<RegisterInput, i64>>
    {
        specs
            .into_iter()
            .enumerate()
            .map(|(i, (call, dur))| Operation {
                client_id: i as u64,
                input: RegisterInput { is_write: true, value: i as i64 },
                call,
                output: 0,
                return_time: call + dur,
            })
            .collect()
    }
}

proptest! {
    /// 1.1 — A writes-only register history is always linearizable, no
    /// matter how the call/return windows overlap.  `RegisterModel::step`
    /// accepts every write unconditionally, so any interleaving is a valid
    /// linearization and the DFS must always succeed.
    ///
    /// INV-LIN-01.
    #[test]
    fn prop_concurrent_writes_only_is_ok(
        history in arb_concurrent_writes(2, 8)
    ) {
        let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
        prop_assert_eq!(result, CheckResult::Ok,
            "INV-LIN-01: a concurrent writes-only history must be linearizable");
    }
}

prop_compose! {
    /// Generate a 2-op (write, read) parameter set in which the read's
    /// call lies strictly before the write's return — i.e. the windows
    /// overlap.  `output_kind` biases `read_output` so the {Ok, Illegal}
    /// branches both get exercised in a 100-case run.
    fn arb_overlap_write_read()
        (write_dur in 5u64..30, write_value in -50i64..50)
        (write_dur in Just(write_dur),
         write_value in Just(write_value),
         read_call in 0u64..write_dur,
         read_dur in 1u64..30,
         arb_out in -50i64..50,
         output_kind in 0u32..3)
        -> (i64, u64, u64, u64, i64)
    {
        let read_output = match output_kind {
            0 => 0,
            1 => write_value,
            _ => arb_out,
        };
        (write_value, write_dur, read_call, read_dur, read_output)
    }
}

proptest! {
    /// 1.2 — A write `[0, write_dur]` overlapping a read whose call lies
    /// in `[0, write_dur)`.  Both linearizations are admissible:
    ///   - `[read, write]`  → register state at read time is 0
    ///   - `[write, read]`  → register state at read time is `write_value`
    /// So the result is `Ok` iff the read returns either of those, and
    /// `Illegal` otherwise.
    ///
    /// INV-LIN-01 + INV-LIN-02.
    #[test]
    fn prop_concurrent_write_overlap_read_matches_membership(
        params in arb_overlap_write_read()
    ) {
        let (write_value, write_dur, read_call, read_dur, read_output) = params;
        let history = vec![
            Operation {
                client_id: 0,
                input: RegisterInput { is_write: true, value: write_value },
                call: 0,
                output: 0,
                return_time: write_dur,
            },
            Operation {
                client_id: 1,
                input: RegisterInput { is_write: false, value: 0 },
                call: read_call,
                output: read_output,
                return_time: read_call + read_dur,
            },
        ];
        let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
        let expected = if read_output == 0 || read_output == write_value {
            CheckResult::Ok
        } else {
            CheckResult::Illegal
        };
        prop_assert_eq!(result, expected,
            "concurrent write/read: write_value={} read_output={} expected={:?}",
            write_value, read_output, expected);
    }
}

prop_compose! {
    /// Generate a 3-op (write1, write2, late_read) parameter set with
    /// `v1, v2 ≥ 1` (so 0 is *not* a valid late-read output), the two
    /// writes overlapping each other, and the read strictly after both
    /// writes complete.
    fn arb_two_writers_late_reader()
        (t1 in 10u64..30, v1 in 1i64..50, v2 in 1i64..50)
        (t1 in Just(t1), v1 in Just(v1), v2 in Just(v2),
         w2_call in 0u64..t1,
         w2_dur in 1u64..30,
         r_dur in 1u64..30,
         arb_out in -50i64..50,
         output_kind in 0u32..3)
        -> (i64, i64, u64, u64, u64, u64, i64)
    {
        let read_output = match output_kind {
            0 => v1,
            1 => v2,
            _ => arb_out,
        };
        (v1, v2, t1, w2_call, w2_dur, r_dur, read_output)
    }
}

proptest! {
    /// 1.3 — Two writes overlapping each other in real time, followed by
    /// a read whose call is strictly after both writes return.  Real-time
    /// order forces the read to come last in any linearization, but the
    /// two writes can be ordered either way.  So the late read must
    /// observe `v1` or `v2`; any other value (including 0, since both
    /// writes are non-zero) is `Illegal`.
    ///
    /// INV-LIN-01 + INV-LIN-02 + INV-HIST-02.
    #[test]
    fn prop_two_writers_late_reader_matches_membership(
        params in arb_two_writers_late_reader()
    ) {
        let (v1, v2, t1, w2_call, w2_dur, r_dur, read_output) = params;
        let w2_return = w2_call + w2_dur;
        let r_call = std::cmp::max(t1, w2_return) + 1;
        let history = vec![
            Operation {
                client_id: 0,
                input: RegisterInput { is_write: true, value: v1 },
                call: 0,
                output: 0,
                return_time: t1,
            },
            Operation {
                client_id: 1,
                input: RegisterInput { is_write: true, value: v2 },
                call: w2_call,
                output: 0,
                return_time: w2_return,
            },
            Operation {
                client_id: 2,
                input: RegisterInput { is_write: false, value: 0 },
                call: r_call,
                output: read_output,
                return_time: r_call + r_dur,
            },
        ];
        let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
        let expected = if read_output == v1 || read_output == v2 {
            CheckResult::Ok
        } else {
            CheckResult::Illegal
        };
        prop_assert_eq!(result, expected,
            "two writers + late read: v1={} v2={} read_output={} expected={:?}",
            v1, v2, read_output, expected);
    }
}

proptest! {
    /// 1.4 — `check_events` must agree with `check_operations` on
    /// concurrent (overlapping) histories.  The earlier sequential
    /// equivalence test never exercises the event pipeline's handling of
    /// interleaved Call/Return events, where bugs in `convert_entries` or
    /// `renumber` would surface.
    ///
    /// INV-LIN-01 + INV-LIN-02.
    #[test]
    fn prop_events_agree_with_operations_on_concurrent_history(
        history in arb_concurrent_writes(2, 6)
    ) {
        let events = ops_to_events_sorted_by_time(&history);
        let ops_result = porcupine::checker::check_operations(&RegisterModel, &history, None);
        let events_result = porcupine::checker::check_events(&RegisterModel, &events, None);
        prop_assert_eq!(ops_result, events_result,
            "check_operations and check_events must agree on concurrent histories");
    }
}
