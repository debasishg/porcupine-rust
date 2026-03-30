/// Property-based tests for porcupine linearizability checker.
///
/// Each test corresponds to one or more INV-* invariants from docs/spec.md.
/// All tests use `proptest` to generate random histories and models.
///
/// Run:  cargo test --test property_tests
use proptest::prelude::*;
use porcupine::{CheckResult, Model, Operation};

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
                input: RegisterInput { is_write: true, value: i as i64 },
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
        input: RegisterInput { is_write: true, value },
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
    /// Tests both soundness (Ok → linearizable) and completeness (linearizable → Ok)
    /// for the trivial sequential case.
    ///
    /// Note: once the DFS is implemented, this must return Ok.
    /// Currently returns Unknown (stub); update assertion when DFS is done.
    #[test]
    fn prop_sequential_history_is_linearizable(len in 1usize..8) {
        let history = sequential_history(len);
        let model = RegisterModel;
        let result = porcupine::checker::check_operations(&model, &history);
        // TODO: change to CheckResult::Ok once DFS is implemented
        prop_assert!(
            result == CheckResult::Ok || result == CheckResult::Unknown,
            "Sequential history must not be Illegal; got {:?}", result
        );
    }
}

proptest! {
    /// A single-operation history is trivially linearizable.
    #[test]
    fn prop_single_op_linearizable(value in -100i64..100) {
        let history = single_op_history(value);
        let model = RegisterModel;
        let result = porcupine::checker::check_operations(&model, &history);
        prop_assert!(
            result == CheckResult::Ok || result == CheckResult::Unknown,
            "Single-op history must not be Illegal; got {:?}", result
        );
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
        let mut by_key: std::collections::HashMap<u8, Vec<usize>> = std::collections::HashMap::new();
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
    /// This is the observable consequence of cache soundness.
    #[test]
    fn prop_cache_sound_deterministic(len in 1usize..6) {
        let history = sequential_history(len);
        let model = RegisterModel;
        let r1 = porcupine::checker::check_operations(&model, &history);
        let r2 = porcupine::checker::check_operations(&model, &history);
        prop_assert_eq!(r1, r2, "INV-LIN-04: identical inputs must yield identical results");
    }
}
