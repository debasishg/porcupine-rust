//! Hegel-driven property tests for the porcupine linearizability checker.
//!
//! Hegel (https://hegel.dev) is Antithesis's universal property-based testing
//! protocol — a Hypothesis-quality generator engine with bindings for several
//! languages. We use the `hegeltest` crate (imports as `hegel`).
//!
//! Coverage:
//!   * INV-HIST-01 — well-formed history (timestamps monotonic per op)
//!   * INV-LIN-01 — soundness: a sequential / single-op history is Ok
//!   * INV-LIN-02 — completeness: a known illegal history is Illegal
//!   * INV-LIN-03 — P-compositionality: partitions are disjoint + complete;
//!     end-to-end agreement on a partitionable model
//!   * INV-LIN-04 — cache soundness: identical inputs yield identical results
//!   * INV-ND-01 — PowerSetModel reduction: degenerate ND model agrees with
//!     its deterministic counterpart; ND sequential writes are Ok; impossible
//!     reads are Illegal
//!   * Cross-API — check_events agrees with check_operations on sequential
//!     histories
//!   * Extras — empty history Ok; prefix-closure of sequential history;
//!     partition idempotence; incremental linearizability via a Hegel state
//!     machine
//!
//! Run:  cargo test --test hegel_properties
//!
//! Hegel will download a private copy of `uv` on first use if `uv` is not on
//! your `PATH` (see https://hegel.dev/reference/installation).

use std::collections::{HashMap, HashSet};

use hegel::TestCase;
use hegel::generators as gs;

use porcupine::{CheckResult, Event, Model, Operation, model::PowerSetModel};

mod common;

use common::{
    DeterministicNdRegister, KvInput, KvModel, LossyInput, LossyNdRegister, RegisterInput,
    RegisterModel, ops_to_events_sorted_by_time, sequential_ops_to_events,
};

// ===========================================================================
// File-local models — only used by the partition-idempotence test below.
// ===========================================================================

/// A wrapper around `KvModel` whose `partition` always returns a single
/// partition containing every index. Used by
/// `hegel_partition_idempotent_with_single_partition` to verify that one
/// whole-history partition agrees with no partition at all.
#[derive(Clone)]
struct KvSinglePartition;

impl Model for KvSinglePartition {
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
        Some(vec![(0..history.len()).collect()])
    }
}

// ===========================================================================
// Hegel composite generators
// ===========================================================================

/// Generate a sequential register history of length `len` with arbitrary
/// payload values. All ops are writes; outputs are ignored.
#[hegel::composite]
fn gen_sequential_history(tc: TestCase) -> Vec<Operation<RegisterInput, i64>> {
    let len = tc.draw(gs::integers::<usize>().min_value(0).max_value(8));
    let mut ts = 0u64;
    let mut ops = Vec::with_capacity(len);
    for i in 0..len {
        let value = tc.draw(gs::integers::<i64>().min_value(-100).max_value(100));
        let duration = tc.draw(gs::integers::<u64>().min_value(1).max_value(10));
        let call = ts;
        let return_time = ts + duration;
        ts = return_time + 1;
        ops.push(Operation {
            client_id: i as u64,
            input: RegisterInput {
                is_write: true,
                value,
            },
            call,
            output: 0,
            return_time,
        });
    }
    ops
}

#[hegel::composite]
fn gen_kv_sequential_history(tc: TestCase) -> Vec<Operation<KvInput, i64>> {
    let len = tc.draw(gs::integers::<usize>().min_value(0).max_value(10));
    let mut ts = 0u64;
    let mut ops = Vec::with_capacity(len);
    for i in 0..len {
        let key = tc.draw(gs::integers::<u8>().min_value(0).max_value(3));
        let value = tc.draw(gs::integers::<i64>().min_value(0).max_value(20));
        let duration = tc.draw(gs::integers::<u64>().min_value(1).max_value(5));
        let call = ts;
        let return_time = ts + duration;
        ts = return_time + 1;
        ops.push(Operation {
            client_id: i as u64,
            input: KvInput {
                key,
                is_write: true,
                value,
            },
            call,
            output: 0,
            return_time,
        });
    }
    ops
}

// ===========================================================================
// INV-HIST-01: well-formed history
// ===========================================================================

#[hegel::test]
fn hegel_well_formed_history(tc: TestCase) {
    let history = tc.draw(gen_sequential_history());
    for op in &history {
        assert!(
            op.call <= op.return_time,
            "INV-HIST-01: call {} > return_time {}",
            op.call,
            op.return_time
        );
    }
}

// ===========================================================================
// INV-LIN-01 + INV-LIN-02: sequential and single-op histories are linearizable
// ===========================================================================

#[hegel::test]
fn hegel_sequential_history_is_linearizable(tc: TestCase) {
    let history = tc.draw(gen_sequential_history());
    let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
    assert_eq!(
        result,
        CheckResult::Ok,
        "INV-LIN-01: sequential history must be linearizable"
    );
}

#[hegel::test]
fn hegel_single_op_is_linearizable(tc: TestCase) {
    let value = tc.draw(gs::integers::<i64>().min_value(-100).max_value(100));
    let history = vec![Operation {
        client_id: 0,
        input: RegisterInput {
            is_write: true,
            value,
        },
        call: 0,
        output: 0,
        return_time: 10,
    }];
    let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
    assert_eq!(
        result,
        CheckResult::Ok,
        "INV-LIN-01: single-op history must be linearizable"
    );
}

#[hegel::test]
fn hegel_empty_history_is_ok(_tc: TestCase) {
    let history: Vec<Operation<RegisterInput, i64>> = vec![];
    let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
    assert_eq!(result, CheckResult::Ok, "empty history must be Ok");

    let events: Vec<Event<RegisterInput, i64>> = vec![];
    let result = porcupine::checker::check_events(&RegisterModel, &events, None);
    assert_eq!(result, CheckResult::Ok, "empty event history must be Ok");
}

/// Every prefix of a sequential history is itself a sequential history, hence
/// linearizable. This is a stronger statement than "the full history is Ok".
#[hegel::test]
fn hegel_prefixes_of_sequential_are_linearizable(tc: TestCase) {
    let history = tc.draw(gen_sequential_history());
    if history.is_empty() {
        return;
    }
    let cut = tc.draw(
        gs::integers::<usize>()
            .min_value(0)
            .max_value(history.len()),
    );
    let prefix = &history[..cut];
    let result = porcupine::checker::check_operations(&RegisterModel, prefix, None);
    assert_eq!(
        result,
        CheckResult::Ok,
        "any prefix of a sequential history is linearizable (length {})",
        cut
    );
}

// ===========================================================================
// INV-LIN-02: a known illegal history is detected
// ===========================================================================

#[hegel::test]
fn hegel_illegal_history_is_detected(_tc: TestCase) {
    let history = vec![
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
        // Read of 0 starts strictly after the write completes — illegal.
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
    ];
    let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
    assert_eq!(
        result,
        CheckResult::Illegal,
        "INV-LIN-02: known non-linearizable history must be Illegal"
    );
}

/// Generative version: we synthesise a stale-read pattern with arbitrary
/// value/timing parameters. Any history of the shape
///   write(v != 0) [0, t1]
///   read → 0     [call > t1, _]
/// is illegal because the read begins after the write completes.
#[hegel::test]
fn hegel_stale_read_is_always_illegal(tc: TestCase) {
    let value = tc.draw(gs::integers::<i64>().min_value(1).max_value(100));
    let write_dur = tc.draw(gs::integers::<u64>().min_value(1).max_value(20));
    let read_gap = tc.draw(gs::integers::<u64>().min_value(1).max_value(20));
    let read_dur = tc.draw(gs::integers::<u64>().min_value(1).max_value(20));

    let write_ret = write_dur;
    let read_call = write_ret + read_gap;
    let read_ret = read_call + read_dur;

    let history = vec![
        Operation {
            client_id: 0,
            input: RegisterInput {
                is_write: true,
                value,
            },
            call: 0,
            output: 0,
            return_time: write_ret,
        },
        Operation {
            client_id: 1,
            input: RegisterInput {
                is_write: false,
                value: 0,
            },
            call: read_call,
            output: 0, // stale read of the initial value
            return_time: read_ret,
        },
    ];
    assert_eq!(
        porcupine::checker::check_operations(&RegisterModel, &history, None),
        CheckResult::Illegal,
        "stale read after a non-zero write completes must be Illegal"
    );
}

// ===========================================================================
// INV-LIN-03: P-compositionality
// ===========================================================================

#[hegel::test]
fn hegel_partitions_are_disjoint_and_complete(tc: TestCase) {
    let history = tc.draw(gen_kv_sequential_history());
    if let Some(parts) = KvModel.partition(&history) {
        let mut seen = HashSet::new();
        for partition in &parts {
            for &idx in partition {
                assert!(
                    seen.insert(idx),
                    "INV-LIN-03: index {} appears in multiple partitions",
                    idx
                );
                assert!(idx < history.len(), "INV-LIN-03: out-of-bounds index");
            }
        }
        assert_eq!(
            seen.len(),
            history.len(),
            "INV-LIN-03: partition does not cover all operations"
        );
    }
}

#[hegel::test]
fn hegel_kv_sequential_history_is_linearizable(tc: TestCase) {
    let history = tc.draw(gen_kv_sequential_history());
    let result = porcupine::checker::check_operations(&KvModel, &history, None);
    assert_eq!(
        result,
        CheckResult::Ok,
        "INV-LIN-03: sequential KV history must be linearizable"
    );
}

/// Idempotence of the partition machinery: a model whose `partition` returns
/// a single all-encompassing partition must produce the same result as a model
/// that returns no partition at all (since both reduce to "check the whole
/// history as one unit").
#[hegel::test]
fn hegel_partition_idempotent_with_single_partition(tc: TestCase) {
    let history = tc.draw(gen_kv_sequential_history());
    let whole = porcupine::checker::check_operations(&KvSinglePartition, &history, None);
    // Use a model with the same step but no partition function.
    #[derive(Clone)]
    struct KvNoPartition;
    impl Model for KvNoPartition {
        type State = HashMap<u8, i64>;
        type Input = KvInput;
        type Output = i64;
        fn init(&self) -> Self::State {
            HashMap::new()
        }
        fn step(
            &self,
            state: &Self::State,
            input: &KvInput,
            output: &i64,
        ) -> Option<Self::State> {
            KvModel.step(state, input, output)
        }
    }
    let direct = porcupine::checker::check_operations(&KvNoPartition, &history, None);
    assert_eq!(
        whole, direct,
        "INV-LIN-03: a single all-indices partition must agree with no partition"
    );
}

// ===========================================================================
// INV-LIN-04: cache soundness — identical inputs → identical results
// ===========================================================================

#[hegel::test]
fn hegel_cache_sound_deterministic_ops(tc: TestCase) {
    let history = tc.draw(gen_sequential_history());
    let r1 = porcupine::checker::check_operations(&RegisterModel, &history, None);
    let r2 = porcupine::checker::check_operations(&RegisterModel, &history, None);
    assert_eq!(r1, r2, "INV-LIN-04: identical inputs must yield identical results");
}

#[hegel::test]
fn hegel_cache_sound_deterministic_events(tc: TestCase) {
    let history = tc.draw(gen_sequential_history());
    let events = sequential_ops_to_events(&history);
    let r1 = porcupine::checker::check_events(&RegisterModel, &events, None);
    let r2 = porcupine::checker::check_events(&RegisterModel, &events, None);
    assert_eq!(
        r1, r2,
        "INV-LIN-04: identical event inputs must yield identical results"
    );
}

// ===========================================================================
// Cross-API: check_events agrees with check_operations on sequential histories
// ===========================================================================

#[hegel::test]
fn hegel_events_agree_with_operations(tc: TestCase) {
    let history = tc.draw(gen_sequential_history());
    let events = sequential_ops_to_events(&history);
    let ops_result = porcupine::checker::check_operations(&RegisterModel, &history, None);
    let events_result = porcupine::checker::check_events(&RegisterModel, &events, None);
    assert_eq!(
        ops_result, events_result,
        "check_operations and check_events must agree on the same sequential history"
    );
}

// ===========================================================================
// Concurrent (overlapping) histories
//
// The properties above almost exclusively use sequential (non-overlapping)
// histories.  The DFS backtracking and cache pruning paths in `checker.rs`
// only fire when ops overlap — these tests force the checker into those
// paths so that bugs in `lift`/`unlift`, `match_idx` wiring, or the
// deferred-clone cache probe become visible.
// ===========================================================================

/// Generate a writes-only register history of length 2..=8 with call/duration
/// drawn independently in [0, 40] / [1, 40] — windows overlap with high
/// probability.
#[hegel::composite]
fn gen_concurrent_writes_history(tc: TestCase) -> Vec<Operation<RegisterInput, i64>> {
    let n = tc.draw(gs::integers::<usize>().min_value(2).max_value(8));
    let mut ops = Vec::with_capacity(n);
    for i in 0..n {
        let call = tc.draw(gs::integers::<u64>().min_value(0).max_value(40));
        let duration = tc.draw(gs::integers::<u64>().min_value(1).max_value(40));
        ops.push(Operation {
            client_id: i as u64,
            input: RegisterInput {
                is_write: true,
                value: i as i64,
            },
            call,
            output: 0,
            return_time: call + duration,
        });
    }
    ops
}

/// 1.1 — A writes-only register history is always linearizable, regardless
/// of how the windows overlap.  `RegisterModel::step` accepts every write,
/// so any DFS interleaving succeeds.
///
/// INV-LIN-01.
#[hegel::test]
fn hegel_concurrent_writes_only_is_ok(tc: TestCase) {
    let history = tc.draw(gen_concurrent_writes_history());
    let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
    assert_eq!(
        result,
        CheckResult::Ok,
        "INV-LIN-01: a concurrent writes-only history must be linearizable"
    );
}

/// 1.2 — A write `[0, write_dur]` overlapping a read whose call lies in
/// `[0, write_dur)`.  Both linearizations are admissible, so the result is
/// `Ok` iff the read returns 0 (linearized before the write) or
/// `write_value` (linearized after).  Anything else is `Illegal`.
///
/// INV-LIN-01 + INV-LIN-02.
#[hegel::test]
fn hegel_concurrent_write_overlap_read_matches_membership(tc: TestCase) {
    let write_dur = tc.draw(gs::integers::<u64>().min_value(5).max_value(30));
    let write_value = tc.draw(gs::integers::<i64>().min_value(-50).max_value(50));
    let read_call = tc.draw(
        gs::integers::<u64>()
            .min_value(0)
            .max_value(write_dur - 1),
    );
    let read_dur = tc.draw(gs::integers::<u64>().min_value(1).max_value(30));
    let arb_out = tc.draw(gs::integers::<i64>().min_value(-50).max_value(50));
    let output_kind = tc.draw(gs::integers::<u32>().min_value(0).max_value(2));
    let read_output = match output_kind {
        0 => 0,
        1 => write_value,
        _ => arb_out,
    };

    let history = vec![
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
    ];
    let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
    let expected = if read_output == 0 || read_output == write_value {
        CheckResult::Ok
    } else {
        CheckResult::Illegal
    };
    assert_eq!(
        result, expected,
        "concurrent write/read: write_value={} read_output={} expected={:?}",
        write_value, read_output, expected
    );
}

/// 1.3 — Two writes overlapping each other in real time, followed by a
/// read whose call is strictly after both writes return.  Real-time order
/// forces the read to come last in every valid linearization, so it must
/// observe `v1` or `v2`.  Both writes are non-zero, so an output of 0
/// (the initial state) is `Illegal`.
///
/// INV-LIN-01 + INV-LIN-02 + INV-HIST-02.
#[hegel::test]
fn hegel_two_writers_late_reader_matches_membership(tc: TestCase) {
    let t1 = tc.draw(gs::integers::<u64>().min_value(10).max_value(30));
    let w2_call = tc.draw(gs::integers::<u64>().min_value(0).max_value(t1 - 1));
    let w2_dur = tc.draw(gs::integers::<u64>().min_value(1).max_value(30));
    let r_dur = tc.draw(gs::integers::<u64>().min_value(1).max_value(30));
    let v1 = tc.draw(gs::integers::<i64>().min_value(1).max_value(50));
    let v2 = tc.draw(gs::integers::<i64>().min_value(1).max_value(50));
    let arb_out = tc.draw(gs::integers::<i64>().min_value(-50).max_value(50));
    let output_kind = tc.draw(gs::integers::<u32>().min_value(0).max_value(2));
    let read_output = match output_kind {
        0 => v1,
        1 => v2,
        _ => arb_out,
    };

    let w2_return = w2_call + w2_dur;
    let r_call = std::cmp::max(t1, w2_return) + 1;
    let history = vec![
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
    ];
    let result = porcupine::checker::check_operations(&RegisterModel, &history, None);
    let expected = if read_output == v1 || read_output == v2 {
        CheckResult::Ok
    } else {
        CheckResult::Illegal
    };
    assert_eq!(
        result, expected,
        "two writers + late read: v1={} v2={} read_output={} expected={:?}",
        v1, v2, read_output, expected
    );
}

/// 1.4 — `check_events` and `check_operations` must agree on concurrent
/// histories.  This is the strongest property for the event pipeline: any
/// bug in `convert_entries`, `renumber`, or the equal-timestamp
/// Call-before-Return tiebreak that causes the two APIs to diverge will
/// be caught here.
///
/// INV-LIN-01 + INV-LIN-02.
#[hegel::test]
fn hegel_events_agree_with_operations_on_concurrent_history(tc: TestCase) {
    let history = tc.draw(gen_concurrent_writes_history());
    let events = ops_to_events_sorted_by_time(&history);
    let ops_result = porcupine::checker::check_operations(&RegisterModel, &history, None);
    let events_result = porcupine::checker::check_events(&RegisterModel, &events, None);
    assert_eq!(
        ops_result, events_result,
        "check_operations and check_events must agree on concurrent histories"
    );
}

// ===========================================================================
// INV-ND-01: Power-Set reduction soundness
// ===========================================================================

#[hegel::test]
fn hegel_nd_deterministic_agrees_with_model(tc: TestCase) {
    let history = tc.draw(gen_sequential_history());
    let det = porcupine::checker::check_operations(&RegisterModel, &history, None);
    let nd_model = PowerSetModel(DeterministicNdRegister);
    let nd = porcupine::checker::check_operations(&nd_model, &history, None);
    assert_eq!(
        det, nd,
        "INV-ND-01: PowerSetModel(deterministic ND) must agree with the deterministic Model"
    );
}

#[hegel::test]
fn hegel_nd_sequential_writes_linearizable(tc: TestCase) {
    let len = tc.draw(gs::integers::<usize>().min_value(0).max_value(8));
    let history: Vec<Operation<LossyInput, Option<i64>>> = (0..len)
        .map(|i| Operation {
            client_id: i as u64,
            input: LossyInput::Write(i as i64),
            output: None,
            call: (i as u64) * 2,
            return_time: (i as u64) * 2 + 1,
        })
        .collect();
    let model = PowerSetModel(LossyNdRegister);
    let result = porcupine::checker::check_operations(&model, &history, None);
    assert_eq!(
        result,
        CheckResult::Ok,
        "INV-ND-01: sequential ND writes must be linearizable"
    );
}

#[hegel::test]
fn hegel_nd_impossible_read_is_illegal(tc: TestCase) {
    let written = tc.draw(gs::integers::<i64>().min_value(1).max_value(100));
    // Read returns a value that was never written and is not 0 (the initial
    // state). Both branches of the lossy step (write succeeded → `written`,
    // write lost → 0) reject it.
    let observed = tc.draw(
        gs::integers::<i64>()
            .min_value(-1000)
            .max_value(-1),
    );
    let history = vec![
        Operation {
            client_id: 0,
            input: LossyInput::Write(written),
            output: None,
            call: 0,
            return_time: 5,
        },
        Operation {
            client_id: 1,
            input: LossyInput::Read,
            output: Some(observed),
            call: 6,
            return_time: 10,
        },
    ];
    let model = PowerSetModel(LossyNdRegister);
    assert_eq!(
        porcupine::checker::check_operations(&model, &history, None),
        CheckResult::Illegal,
        "INV-ND-01: a read of a value reachable in no branch must be Illegal"
    );
}

// ===========================================================================
// Stateful: incremental linearizability via a Hegel state machine
//
// We grow a sequential register history one op at a time. After every step the
// checker must report Ok — this exercises soundness incrementally and surfaces
// any bug whose effect depends on history length or interleaving order.
// ===========================================================================

struct IncrementalRegister {
    history: Vec<Operation<RegisterInput, i64>>,
    next_ts: u64,
}

#[hegel::state_machine]
impl IncrementalRegister {
    #[rule]
    fn append_write(&mut self, tc: TestCase) {
        let value = tc.draw(gs::integers::<i64>().min_value(-50).max_value(50));
        let duration = tc.draw(gs::integers::<u64>().min_value(1).max_value(10));
        let call = self.next_ts;
        let return_time = call + duration;
        self.next_ts = return_time + 1;
        self.history.push(Operation {
            client_id: self.history.len() as u64,
            input: RegisterInput {
                is_write: true,
                value,
            },
            call,
            output: 0,
            return_time,
        });
        let result = porcupine::checker::check_operations(&RegisterModel, &self.history, None);
        assert_eq!(
            result,
            CheckResult::Ok,
            "incremental sequential history must remain linearizable (len={})",
            self.history.len()
        );
    }

    #[rule]
    fn append_read_of_last(&mut self, tc: TestCase) {
        let last_value = self
            .history
            .iter()
            .rev()
            .find_map(|op| op.input.is_write.then_some(op.input.value))
            .unwrap_or(0);
        let duration = tc.draw(gs::integers::<u64>().min_value(1).max_value(5));
        let call = self.next_ts;
        let return_time = call + duration;
        self.next_ts = return_time + 1;
        self.history.push(Operation {
            client_id: self.history.len() as u64,
            input: RegisterInput {
                is_write: false,
                value: 0,
            },
            call,
            output: last_value,
            return_time,
        });
        let result = porcupine::checker::check_operations(&RegisterModel, &self.history, None);
        assert_eq!(
            result,
            CheckResult::Ok,
            "read of last written value in a sequential history must be Ok"
        );
    }
}

#[hegel::test]
fn hegel_incremental_register_is_linearizable(tc: TestCase) {
    let machine = IncrementalRegister {
        history: Vec::new(),
        next_ts: 0,
    };
    hegel::stateful::run(machine, tc);
}
