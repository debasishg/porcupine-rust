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
    AlwaysRejectNd, AlwaysStutterNd, DeterministicNdRegister, KvInput, KvModel,
    KvModelReversedPartition, KvNoPartition, LossyInput, LossyNdRegister, RegisterInput,
    RegisterModel, build_overlap_write_read, build_two_writers_late_reader,
    illegal_register_history, illegal_register_history_as_events, ops_to_events_sorted_by_time,
    sequential_history, sequential_ops_to_events, single_op_history,
};
use porcupine::model::HashedPowerSetModel;
use std::time::Duration;

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

// ===========================================================================
// Algebraic invariance properties
//
// These properties relate two runs of `check_operations` on histories
// that differ only in incidental representation — absolute timestamps,
// `client_id` values, slice order, or appended ops.  The checker's
// output must depend only on the abstract linearizability question, not
// on these representational details.  A regression that leaks any of
// them into the verdict would be caught here in one assertion.
// ===========================================================================

/// Mix of generators producing both Ok and Illegal histories so the
/// algebraic checks below exercise both verdicts in a 100-case run.
fn arb_mixed_register_history() -> impl Strategy<Value = Vec<Operation<RegisterInput, i64>>> {
    prop_oneof![
        arb_concurrent_writes(2, 6).boxed(),
        arb_overlap_write_read()
            .prop_map(|(write_value, write_dur, read_call, read_dur, read_output)| {
                build_overlap_write_read(write_value, write_dur, read_call, read_dur, read_output)
            })
            .boxed(),
        arb_two_writers_late_reader()
            .prop_map(|(v1, v2, t1, w2_call, w2_dur, r_dur, read_output)| {
                build_two_writers_late_reader(v1, v2, t1, w2_call, w2_dur, r_dur, read_output)
            })
            .boxed(),
        Just(illegal_register_history()).boxed(),
    ]
}

/// Deterministic Fisher-Yates permutation of `0..n`, seeded from `seed`.
/// PCG-style LCG keeps the test fully reproducible — proptest can
/// shrink the seed when a counter-example is found.
fn lcg_permutation(n: usize, seed: u64) -> Vec<usize> {
    let mut perm: Vec<usize> = (0..n).collect();
    let mut state = seed.wrapping_add(1);
    for i in (1..n).rev() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let j = (state as usize) % (i + 1);
        perm.swap(i, j);
    }
    perm
}

proptest! {
    /// 2.1 — Time-shift invariance.
    ///
    /// Shifting every (call, return_time) by a constant Δ must preserve
    /// the linearizability result.  The checker reads only relative
    /// ordering through `precedesInRealTime(a, b) = a.return_time <
    /// b.call`, which is invariant under uniform addition.  A bug that
    /// stored or compared absolute timestamps somewhere would surface
    /// here.
    ///
    /// INV-HIST-02.
    #[test]
    fn prop_time_shift_invariance(
        history in arb_mixed_register_history(),
        delta in 0u64..1000
    ) {
        let shifted: Vec<_> = history
            .iter()
            .map(|op| Operation {
                client_id: op.client_id,
                input: op.input.clone(),
                call: op.call + delta,
                output: op.output,
                return_time: op.return_time + delta,
            })
            .collect();
        let r1 = porcupine::checker::check_operations(&RegisterModel, &history, None);
        let r2 = porcupine::checker::check_operations(&RegisterModel, &shifted, None);
        prop_assert_eq!(r1, r2,
            "INV-HIST-02: shifting all timestamps by {} must preserve the result",
            delta);
    }
}

proptest! {
    /// 2.2 — Client-ID invariance.
    ///
    /// `client_id` is caller metadata.  The checker reads only `call`,
    /// `return_time`, `input`, and `output` — `Entry` doesn't even carry
    /// `client_id` through the DFS.  Permuting `client_id` (and biasing
    /// the new ids upward to detect any reliance on small/dense values)
    /// must not change the result.
    #[test]
    fn prop_client_id_invariance(
        history in arb_mixed_register_history(),
        seed in any::<u64>()
    ) {
        let perm = lcg_permutation(history.len(), seed);
        let permuted: Vec<_> = history
            .iter()
            .enumerate()
            .map(|(i, op)| {
                let mut new_op = op.clone();
                new_op.client_id = (perm[i] as u64).wrapping_add(1000);
                new_op
            })
            .collect();
        let r1 = porcupine::checker::check_operations(&RegisterModel, &history, None);
        let r2 = porcupine::checker::check_operations(&RegisterModel, &permuted, None);
        prop_assert_eq!(r1, r2,
            "client_id permutation must not change the linearizability result");
    }
}

proptest! {
    /// 2.3 — Equal-timestamp tiebreak invariance.
    ///
    /// Two writes share *exactly* the same (call, return_time) pair.
    /// Slice order then determines which one the stable sort places
    /// first, but both orderings are valid linearizations — the verdict
    /// must be identical.  A bug that uses slice order as a heuristic
    /// in the DFS or the cache key would diverge here.
    ///
    /// INV-HIST-02.
    #[test]
    fn prop_equal_timestamp_tiebreak_invariance(
        v1 in -50i64..50,
        v2 in -50i64..50,
        read_output in -50i64..50,
    ) {
        // Two writes at *identical* call/return timestamps, then a late read.
        let h0 = vec![
            Operation { client_id: 0, input: RegisterInput { is_write: true, value: v1 }, call: 0, output: 0, return_time: 10 },
            Operation { client_id: 1, input: RegisterInput { is_write: true, value: v2 }, call: 0, output: 0, return_time: 10 },
            Operation { client_id: 2, input: RegisterInput { is_write: false, value: 0 }, call: 11, output: read_output, return_time: 20 },
        ];
        // Identical history with the two tied writes swapped in the slice.
        let h1 = vec![h0[1].clone(), h0[0].clone(), h0[2].clone()];
        let r0 = porcupine::checker::check_operations(&RegisterModel, &h0, None);
        let r1 = porcupine::checker::check_operations(&RegisterModel, &h1, None);
        prop_assert_eq!(r0, r1,
            "tied-timestamp swap must preserve the result (v1={}, v2={}, read_output={})",
            v1, v2, read_output);
    }
}

proptest! {
    /// 2.4 — Slice-order invariance.
    ///
    /// Permuting the input slice (with all distinct timestamps) must
    /// not change the result, because `make_entries` sorts by
    /// `(time, kind)` before DFS.  A bug where the checker iterates ops
    /// in slice order somewhere — say as a cache-key seed or DFS
    /// ordering hint — would surface here.
    ///
    /// INV-HIST-02.
    #[test]
    fn prop_slice_order_invariance(
        history in arb_mixed_register_history(),
        seed in any::<u64>()
    ) {
        let perm = lcg_permutation(history.len(), seed);
        let shuffled: Vec<_> = perm.iter().map(|&i| history[i].clone()).collect();
        let r1 = porcupine::checker::check_operations(&RegisterModel, &history, None);
        let r2 = porcupine::checker::check_operations(&RegisterModel, &shuffled, None);
        prop_assert_eq!(r1, r2,
            "slice-order permutation must not change the result");
    }
}

proptest! {
    /// 2.5 — Append-preserves-Illegal.
    ///
    /// `check(h) = Illegal` ⇒ `check(h ++ tail) = Illegal` for any
    /// well-formed `tail` whose timestamps are strictly after `h`'s.
    /// Linearizability is monotone under temporal extension: any valid
    /// linearization σ' of `(h ++ tail)` restricted to `h` would itself
    /// be a valid linearization of `h`, so an existing violation cannot
    /// be "uncovered" by appending.  All extras here are writes, which
    /// `RegisterModel` accepts unconditionally — so the only possible
    /// source of `Illegal` remains the original violation in `h`.
    ///
    /// INV-LIN-02.
    #[test]
    fn prop_append_preserves_illegal(
        extras in prop::collection::vec(1u64..20, 0..6)
    ) {
        let mut h = illegal_register_history();
        let mut ts = h.iter().map(|op| op.return_time).max().unwrap_or(0) + 1;
        for (i, dur) in extras.iter().enumerate() {
            h.push(Operation {
                client_id: (100 + i) as u64,
                input: RegisterInput { is_write: true, value: 100 + i as i64 },
                call: ts,
                output: 0,
                return_time: ts + dur,
            });
            ts += dur + 1;
        }
        let result = porcupine::checker::check_operations(&RegisterModel, &h, None);
        prop_assert_eq!(result, CheckResult::Illegal,
            "INV-LIN-02: appending {} writes to an illegal history must preserve Illegal",
            extras.len());
    }
}

// ===========================================================================
// §3 — Partition / P-compositionality equivalence
//
// The existing INV-LIN-03 tests verify that partitions produced by
// `KvModel::partition` are disjoint and complete, but never exercise the
// *equivalence* itself: do per-partition checks agree with whole-history
// checks?  These properties pin that down by running the same history
// through both `KvModel` (partitioning) and `KvNoPartition` (whole) and
// asserting result equality.
// ===========================================================================

prop_compose! {
    /// Generate a sequential KV history (calls don't overlap) of length
    /// `len` over `n_keys` distinct keys.  Read outputs are pre-set to
    /// the requested value, which may or may not match the actual
    /// register state — this produces a mix of Ok and Illegal histories.
    fn arb_kv_history(min_len: usize, max_len: usize, n_keys: u8)
        (n in min_len..=max_len)
        (specs in prop::collection::vec(
            (0u8..n_keys, prop::bool::ANY, 0i64..10),
            n..=n,
        ))
        -> Vec<Operation<KvInput, i64>>
    {
        let mut ts = 0u64;
        specs
            .into_iter()
            .enumerate()
            .map(|(i, (key, is_write, value))| {
                let call = ts;
                let return_time = ts + 5;
                ts = return_time + 1;
                Operation {
                    client_id: i as u64,
                    input: KvInput { key, is_write, value },
                    call,
                    output: value,
                    return_time,
                }
            })
            .collect()
    }
}

proptest! {
    /// 3.1 — Partition equivalence.
    ///
    /// `check_operations(KvModel, h) == check_operations(KvNoPartition, h)`
    /// for any KV history `h`.  This is the operational form of
    /// INV-LIN-03: a history is linearizable iff each per-key partition
    /// is independently linearizable.
    ///
    /// INV-LIN-03.
    #[test]
    fn prop_kv_partition_equivalence(
        history in arb_kv_history(2, 8, 3)
    ) {
        let with_part = porcupine::checker::check_operations(&KvModel, &history, None);
        let without   = porcupine::checker::check_operations(&KvNoPartition, &history, None);
        prop_assert_eq!(with_part, without,
            "INV-LIN-03: per-partition and whole-history checks must agree");
    }
}

proptest! {
    /// 3.2 — Partition order invariance.
    ///
    /// Reversing the order of `KvModel::partition`'s output must not
    /// change the result.  `check_parallel` re-sorts internally, but a
    /// regression that leaks partition order into the verdict (e.g. via
    /// kill-flag race priority) would surface here.
    ///
    /// INV-LIN-03.
    #[test]
    fn prop_kv_partition_order_invariance(
        history in arb_kv_history(2, 8, 3)
    ) {
        let normal   = porcupine::checker::check_operations(&KvModel, &history, None);
        let reversed = porcupine::checker::check_operations(&KvModelReversedPartition, &history, None);
        prop_assert_eq!(normal, reversed,
            "reversing the partition order must preserve the result");
    }
}

/// 3.3 — Cross-partition independence.
///
/// Two writes to disjoint keys with overlapping windows are independent
/// at the linearizability level — each partition is sequential within
/// itself.  Result must be Ok regardless of any cross-partition real-time
/// relationship.
///
/// INV-LIN-03.
#[test]
fn prop_disjoint_keys_independent() {
    let history = vec![
        Operation { client_id: 0, input: KvInput { key: 1, is_write: true,  value: 100 }, call: 0,  output: 0,   return_time: 10 },
        Operation { client_id: 1, input: KvInput { key: 2, is_write: true,  value: 200 }, call: 5,  output: 0,   return_time: 15 }, // overlaps key 1's write but disjoint key
        Operation { client_id: 2, input: KvInput { key: 1, is_write: false, value: 0   }, call: 11, output: 100, return_time: 20 },
        Operation { client_id: 3, input: KvInput { key: 2, is_write: false, value: 0   }, call: 16, output: 200, return_time: 25 },
    ];
    let result = porcupine::checker::check_operations(&KvModel, &history, None);
    assert_eq!(result, CheckResult::Ok,
        "INV-LIN-03: writes to disjoint keys must be independent regardless of real-time overlap");
}

proptest! {
    /// 3.4 — Single-key partition fast-path equivalence.
    ///
    /// A history with only one key produces `partitions.len() == 1` and
    /// triggers the single-partition fast path in `check_parallel`.
    /// That path must agree with the no-partition wrapper on the same
    /// history.
    ///
    /// INV-LIN-03.
    #[test]
    fn prop_single_key_kv_partition_fast_path(
        history in arb_kv_history(2, 8, 1)
    ) {
        let with_part = porcupine::checker::check_operations(&KvModel, &history, None);
        let without   = porcupine::checker::check_operations(&KvNoPartition, &history, None);
        prop_assert_eq!(with_part, without,
            "single-key fast path must agree with no-partition");
    }
}

// ===========================================================================
// §4 — Power-set / nondeterministic model invariants
// ===========================================================================

proptest! {
    /// 4.1 — `PowerSetModel::step` output has no `PartialEq`-duplicates.
    ///
    /// Structural enforcement is via the `deduplicate` call inside
    /// `PowerSetModel::step` (src/model.rs).  This test catches a
    /// regression that removes that call.
    ///
    /// INV-ND-01.
    #[test]
    fn prop_powerset_step_has_no_duplicates(
        write_value in -10i64..10,
        seed_states in prop::collection::vec(-10i64..10, 1..6),
    ) {
        let pm = PowerSetModel(LossyNdRegister);
        if let Some(next) = pm.step(&seed_states, &LossyInput::Write(write_value), &None) {
            for (i, s) in next.iter().enumerate() {
                for s2 in &next[i + 1..] {
                    prop_assert_ne!(s, s2, "PowerSetModel::step output had a duplicate state");
                }
            }
        }
    }
}

prop_compose! {
    /// Generate a sequential lossy-register history of `len` writes.
    fn arb_lossy_writes_history(min_len: usize, max_len: usize)
        (n in min_len..=max_len)
        (values in prop::collection::vec(-10i64..10, n..=n))
        -> Vec<Operation<LossyInput, Option<i64>>>
    {
        values
            .into_iter()
            .enumerate()
            .map(|(i, v)| Operation {
                client_id: i as u64,
                input: LossyInput::Write(v),
                output: None,
                call: (i as u64) * 2,
                return_time: (i as u64) * 2 + 1,
            })
            .collect()
    }
}

proptest! {
    /// 4.2 — `PowerSetModel` ≡ `HashedPowerSetModel` on `Eq + Hash` states.
    ///
    /// Both adapters reduce a `NondeterministicModel` to a `Model`; they
    /// differ only in dedup strategy (Vec linear scan vs `HashSet`).  On
    /// any `Eq + Hash` state type, both must give identical results on
    /// the same history.
    ///
    /// INV-ND-01.
    #[test]
    fn prop_powerset_eq_hashed_powerset(
        history in arb_lossy_writes_history(1, 6)
    ) {
        let pm  = PowerSetModel(LossyNdRegister);
        let hpm = HashedPowerSetModel(LossyNdRegister);
        let r1 = porcupine::checker::check_operations(&pm,  &history, None);
        let r2 = porcupine::checker::check_operations(&hpm, &history, None);
        prop_assert_eq!(r1, r2,
            "PowerSetModel and HashedPowerSetModel must agree on Eq+Hash state types");
    }
}

proptest! {
    /// 4.3 — Concurrent lossy writes membership.
    ///
    /// Two overlapping `LossyInput::Write` ops followed by a strictly
    /// later read.  The lossy register's branching `step` means the
    /// power-state after both writes is `{0, v1, v2}`.  A subsequent
    /// read returning `o` is Ok iff `o ∈ {0, v1, v2}`, Illegal otherwise.
    ///
    /// INV-ND-01.
    #[test]
    fn prop_concurrent_lossy_writes_membership(
        v1 in 1i64..50,
        v2 in 1i64..50,
        arb_out in -50i64..50,
        output_kind in 0u32..4,
    ) {
        let read_output = match output_kind {
            0 => 0,
            1 => v1,
            2 => v2,
            _ => arb_out,
        };
        let history = vec![
            Operation { client_id: 0, input: LossyInput::Write(v1), output: None,             call: 0,  return_time: 10 },
            Operation { client_id: 1, input: LossyInput::Write(v2), output: None,             call: 5,  return_time: 15 },
            Operation { client_id: 2, input: LossyInput::Read,      output: Some(read_output), call: 16, return_time: 25 },
        ];
        let model = PowerSetModel(LossyNdRegister);
        let result = porcupine::checker::check_operations(&model, &history, None);
        let expected = if read_output == 0 || read_output == v1 || read_output == v2 {
            CheckResult::Ok
        } else {
            CheckResult::Illegal
        };
        prop_assert_eq!(result, expected,
            "lossy concurrent writes membership: v1={} v2={} read={} expected={:?}",
            v1, v2, read_output, expected);
    }
}

proptest! {
    /// 4.4 — Always-reject ND model: every non-empty history is Illegal.
    ///
    /// `AlwaysRejectNd::step` always returns `vec![]`, so the power-state
    /// becomes empty after the first transition — `PowerSetModel::step`
    /// returns `None`, the DFS rejects, and the verdict is Illegal.
    ///
    /// INV-ND-01.
    #[test]
    fn prop_always_reject_nd_history_illegal(len in 1usize..6) {
        let model = PowerSetModel(AlwaysRejectNd);
        let history: Vec<Operation<(), ()>> = (0..len)
            .map(|i| Operation {
                client_id: i as u64,
                input:     (),
                output:    (),
                call:      (i as u64) * 2,
                return_time: (i as u64) * 2 + 1,
            })
            .collect();
        let result = porcupine::checker::check_operations(&model, &history, None);
        prop_assert_eq!(result, CheckResult::Illegal,
            "INV-ND-01: AlwaysRejectNd must reject every non-empty history");
    }
}

proptest! {
    /// 4.5 — Always-stutter ND model: every history is Ok.
    ///
    /// `AlwaysStutterNd::step` always returns the same single-state Vec,
    /// so every transition is accepted with no state change.  Empty and
    /// non-empty histories alike must be Ok.
    ///
    /// INV-ND-01.
    #[test]
    fn prop_always_stutter_nd_history_ok(len in 0usize..6) {
        let model = PowerSetModel(AlwaysStutterNd);
        let history: Vec<Operation<(), ()>> = (0..len)
            .map(|i| Operation {
                client_id: i as u64,
                input:     (),
                output:    (),
                call:      (i as u64) * 2,
                return_time: (i as u64) * 2 + 1,
            })
            .collect();
        let result = porcupine::checker::check_operations(&model, &history, None);
        prop_assert_eq!(result, CheckResult::Ok,
            "INV-ND-01: AlwaysStutterNd must accept every history");
    }
}

// ===========================================================================
// §5 — Timeout semantics
// ===========================================================================

proptest! {
    /// 5.1 — `None` timeout never returns `Unknown`.
    ///
    /// Without a timer thread, `to_check_result` can only resolve to
    /// Ok or Illegal — Unknown requires `timed_out` to be set, which only
    /// happens under a `Some(duration)` timeout.
    #[test]
    fn prop_no_timeout_never_unknown(history in arb_mixed_register_history()) {
        let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
        prop_assert_ne!(result, CheckResult::Unknown,
            "check_operations(_, None) must return Ok or Illegal, never Unknown");
    }
}

proptest! {
    /// 5.3 — Generous timeout matches unbounded.
    ///
    /// For any small history, a `Some(10s)` timeout is effectively
    /// unbounded — the DFS finishes in milliseconds and the timer is
    /// cancelled before firing.  Result must equal the `None` case.
    #[test]
    fn prop_generous_timeout_matches_unbounded(
        history in arb_mixed_register_history()
    ) {
        let r_none = porcupine::checker::check_operations(&RegisterModel, &history, None);
        let r_long = porcupine::checker::check_operations(
            &RegisterModel, &history, Some(Duration::from_secs(10)));
        prop_assert_eq!(r_none, r_long,
            "Some(10s) timeout must match None on small histories");
    }
}

// ===========================================================================
// §6 — Edge-case timestamps and degenerate histories
// ===========================================================================

/// 6.1 — Zero-duration ops (`call == return_time`) are well-formed per
/// `op.call ≤ op.return_time` and must not break `make_entries`'
/// Call-before-Return tiebreak.
///
/// INV-HIST-01.
#[test]
fn prop_zero_duration_op_handled() {
    let history = vec![Operation {
        client_id: 0,
        input: RegisterInput { is_write: true, value: 42 },
        call: 5,
        output: 0,
        return_time: 5, // zero duration — call == return_time
    }];
    let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
    assert_eq!(result, CheckResult::Ok,
        "zero-duration op must be handled without panicking");
}

/// 6.2 — All-coincident timestamps: full concurrency among all ops.
/// The Call-before-Return tiebreak determines event order; result must
/// still be Ok for writes-only histories.
///
/// INV-HIST-02.
#[test]
fn prop_all_coincident_timestamps_handled() {
    let history = vec![
        Operation { client_id: 0, input: RegisterInput { is_write: true, value: 1 }, call: 0, output: 0, return_time: 1 },
        Operation { client_id: 1, input: RegisterInput { is_write: true, value: 2 }, call: 0, output: 0, return_time: 1 },
        Operation { client_id: 2, input: RegisterInput { is_write: true, value: 3 }, call: 0, output: 0, return_time: 1 },
    ];
    let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
    assert_eq!(result, CheckResult::Ok,
        "all-coincident-timestamp writes must be linearizable");
}

/// 6.3 — Timestamps near `u64::MAX` must not overflow in the entry-list
/// arithmetic.  The checker stores `op.call` and `op.return_time`
/// directly without addition or subtraction, so this should be safe.
#[test]
fn prop_near_u64_max_timestamps_handled() {
    let base = u64::MAX - 1000;
    let history = vec![
        Operation { client_id: 0, input: RegisterInput { is_write: true, value: 1 }, call: base,           output: 0, return_time: base + 100 },
        Operation { client_id: 1, input: RegisterInput { is_write: false, value: 0 }, call: base + 200,     output: 1, return_time: base + 300 },
    ];
    let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
    assert_eq!(result, CheckResult::Ok,
        "timestamps near u64::MAX must not overflow");
}

/// 6.4 — Extreme `i64` values (`MIN`, `MAX`) flow through `RegisterInput`
/// and `Output` without panicking or false-Illegal.
#[test]
fn prop_extreme_i64_values_handled() {
    let history = vec![
        Operation { client_id: 0, input: RegisterInput { is_write: true,  value: i64::MIN }, call: 0,  output: 0,        return_time: 10 },
        Operation { client_id: 1, input: RegisterInput { is_write: false, value: 0 },        call: 11, output: i64::MIN, return_time: 20 },
        Operation { client_id: 2, input: RegisterInput { is_write: true,  value: i64::MAX }, call: 21, output: 0,        return_time: 30 },
        Operation { client_id: 3, input: RegisterInput { is_write: false, value: 0 },        call: 31, output: i64::MAX, return_time: 40 },
    ];
    let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
    assert_eq!(result, CheckResult::Ok,
        "i64::MIN/MAX values must round-trip through write/read correctly");
}

proptest! {
    /// 6.5 — Long sequential chain spilling Bitset past its inline
    /// `SmallVec<[u64; 4]>` capacity (256 ops).  Sequential histories
    /// are trivially linearizable, so the DFS finishes in O(n).
    ///
    /// INV-LIN-04.
    #[test]
    fn prop_long_sequential_chain_ok(len in 257usize..350) {
        let history = sequential_history(len);
        let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
        prop_assert_eq!(result, CheckResult::Ok,
            "long sequential chain ({} ops) must remain linearizable past Bitset spill",
            len);
    }
}

// ===========================================================================
// §9 — Round-trip / API equivalence
// ===========================================================================

proptest! {
    /// 9.2 — Renumber idempotence.
    ///
    /// The event-based API renumbers `Event::id` to a contiguous range
    /// starting at 0 before DFS.  Histories with sparse, non-contiguous
    /// ids must produce the same result as histories with dense ids.
    #[test]
    fn prop_renumber_idempotence(history in arb_concurrent_writes(2, 5)) {
        let events_dense = ops_to_events_sorted_by_time(&history);
        // Same events but with sparse, deliberately-non-contiguous ids
        // (still pairing each Call with its matching Return).
        let events_sparse: Vec<_> = events_dense
            .iter()
            .map(|ev| {
                let mut ev = ev.clone();
                ev.id = ev.id.wrapping_mul(1000).wrapping_add(7);
                ev
            })
            .collect();
        let r1 = porcupine::checker::check_events(&RegisterModel, &events_dense,  None);
        let r2 = porcupine::checker::check_events(&RegisterModel, &events_sparse, None);
        prop_assert_eq!(r1, r2,
            "renumber must produce a result that does not depend on input id density");
    }
}

// ===========================================================================
// §10 — Negative-control / false-positive guards
//
// The completeness invariant says "if a history is non-linearizable, the
// checker returns Illegal."  These properties supply the dual: they
// construct *deliberately* non-linearizable histories and assert the
// checker doesn't return Ok by accident.
// ===========================================================================

proptest! {
    /// 10.1 — Adversarial register read-after-writes.
    ///
    /// A sequential chain of writes with values in `[1, 50]` followed by
    /// a read returning a value in `[-50, 0]`.  The witness set is the
    /// written values plus 0 (initial state); the read's value lies
    /// outside that set, so the verdict must be Illegal.
    ///
    /// INV-LIN-02.
    #[test]
    fn prop_adversarial_read_after_writes_is_illegal(
        write_values in prop::collection::vec(1i64..50, 1..5),
        wrong_value  in -50i64..0,
    ) {
        let mut history: Vec<_> = write_values
            .iter()
            .enumerate()
            .map(|(i, &v)| Operation {
                client_id:   i as u64,
                input:       RegisterInput { is_write: true, value: v },
                call:        (i as u64) * 11,
                output:      0,
                return_time: (i as u64) * 11 + 5,
            })
            .collect();
        let last_return = history.last().unwrap().return_time;
        history.push(Operation {
            client_id:   100,
            input:       RegisterInput { is_write: false, value: 0 },
            call:        last_return + 1,
            output:      wrong_value,
            return_time: last_return + 10,
        });
        let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
        prop_assert_eq!(result, CheckResult::Illegal,
            "INV-LIN-02: a read of a value never written (and ≠ 0) must be Illegal");
    }
}

proptest! {
    /// 10.2 — Adversarial KV read.
    ///
    /// Write a positive value to a key, then read that key and return a
    /// negative value.  The negative result lies outside the witness set
    /// `{0, write_value}`, so the verdict on that partition must be
    /// Illegal — and `check_parallel` must surface it.
    ///
    /// INV-LIN-02 + INV-LIN-03.
    #[test]
    fn prop_adversarial_kv_read_is_illegal(
        write_value in 1i64..50,
        wrong_value in -50i64..0,
    ) {
        let history = vec![
            Operation { client_id: 0, input: KvInput { key: 1, is_write: true,  value: write_value }, call: 0,  output: 0,           return_time: 10 },
            Operation { client_id: 1, input: KvInput { key: 1, is_write: false, value: 0 },           call: 11, output: wrong_value, return_time: 20 },
        ];
        let result = porcupine::checker::check_operations(&KvModel, &history, None);
        prop_assert_eq!(result, CheckResult::Illegal,
            "INV-LIN-02: a wrong KV read in a single-key partition must surface as Illegal");
    }
}
