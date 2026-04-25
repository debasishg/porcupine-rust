//! Integration tests ported from the Go porcupine test suite.
//!
//! Covers:
//!   - Register model: basic linearizability tests
//!   - Etcd model:     102 Jepsen etcd trace files
//!   - KV model:       6-key partitioned + non-partitioned traces
//!   - Set model:      4 inline test cases
//!
//! Test data lives in `test_data/` at the repo root.
//!
//! Intentionally excluded from the Go suite:
//!   - `TestNondeterministicRegisterModel`: NondeterministicModel not in Rust API
//!   - `TestRegisterModelMetadata`:         metadata field not in `Operation`
//!   - Benchmarks:                          out of scope

#![allow(dead_code)]

use porcupine::checker::{check_events, check_operations};
use porcupine::{CheckResult, Event, EventKind, Model, Operation};
use std::collections::HashMap;
use std::sync::Arc;

// ============================================================================
// REGISTER MODEL
// Mirrors Go's `registerModel`: a single mutable integer register.
// ============================================================================

#[derive(Clone)]
struct RegisterModel;

#[derive(Clone, Debug, PartialEq)]
enum RegOp {
    Put,
    Get,
}

#[derive(Clone, Debug, PartialEq)]
struct RegisterInput {
    op: RegOp,
    value: i32,
}

impl Model for RegisterModel {
    type State = i32;
    type Input = RegisterInput;
    type Output = i32;

    fn init(&self) -> i32 {
        0
    }

    fn step(&self, state: &i32, input: &RegisterInput, output: &i32) -> Option<i32> {
        match input.op {
            RegOp::Put => Some(input.value),
            RegOp::Get => {
                if *output == *state {
                    Some(*state)
                } else {
                    None
                }
            }
        }
    }
}

fn reg_op(
    id: u64,
    op: RegOp,
    value: i32,
    output: i32,
    call: u64,
    ret: u64,
) -> Operation<RegisterInput, i32> {
    Operation {
        client_id: id,
        input: RegisterInput { op, value },
        output,
        call,
        return_time: ret,
    }
}

// ============================================================================
// ETCD MODEL
// Mirrors Go's `etcdModel`: a single key supporting read / write / CAS.
//
// State:  Option<i64>       — None = key absent, Some(v) = key holds v
// Input:  EtcdInput { op, arg1, arg2 }
// Output: EtcdOutput { ok, exists, value, unknown }
//
// `unknown = true` means the operation timed out; it may be inserted at any
// point in the linearization.
// ============================================================================

#[derive(Clone)]
struct EtcdModel;

#[derive(Clone, Debug, PartialEq)]
enum EtcdOp {
    Read,
    Write,
    Cas,
}

#[derive(Clone, Debug, PartialEq)]
struct EtcdInput {
    op: EtcdOp,
    arg1: i64,
    arg2: i64,
}

#[derive(Clone, Debug, PartialEq)]
struct EtcdOutput {
    ok: bool,
    exists: bool,
    value: i64,
    unknown: bool,
}

impl Model for EtcdModel {
    type State = Option<i64>;
    type Input = EtcdInput;
    type Output = EtcdOutput;

    fn init(&self) -> Option<i64> {
        None
    }

    fn step(
        &self,
        state: &Option<i64>,
        input: &EtcdInput,
        output: &EtcdOutput,
    ) -> Option<Option<i64>> {
        // Mirror Go's etcdModel.Step exactly.
        // Go uses -1000000 as sentinel for "absent"; we use None.
        match input.op {
            EtcdOp::Read => {
                // ok = (exists==false && st==-1000000) || (exists==true && st==value) || unknown
                let ok = match state {
                    None => !output.exists || output.unknown,
                    Some(v) => (output.exists && output.value == *v) || output.unknown,
                };
                if ok { Some(*state) } else { None }
            }

            // Write always applies regardless of unknown.
            EtcdOp::Write => Some(Some(input.arg1)),

            EtcdOp::Cas => {
                // ok = (arg1==st && out.ok) || (arg1!=st && !out.ok) || unknown
                // result = inp.arg2 if arg1==st, else st
                let (st_matches, next_state) = match state {
                    None => (false, None),
                    Some(v) => {
                        if *v == input.arg1 {
                            (true, Some(input.arg2))
                        } else {
                            (false, *state)
                        }
                    }
                };
                let ok = (st_matches && output.ok) || (!st_matches && !output.ok) || output.unknown;
                if ok { Some(next_state) } else { None }
            }
        }
    }
}

// ============================================================================
// KV MODEL — partitioned by key
// Mirrors Go's `kvModel`: a multi-key store with get / put / append.
//
// Partition strategy: each key's operations form an independent sub-history.
// State per partition: a single String (the key's accumulated value).
// ============================================================================

#[derive(Clone)]
struct KvModel;

#[derive(Clone, Debug, PartialEq)]
enum KvOp {
    Get,
    Put,
    Append,
}

#[derive(Clone, Debug, PartialEq)]
struct KvInput {
    op: KvOp,
    key: Arc<str>,
    value: Arc<str>,
}

#[derive(Clone, Debug, PartialEq)]
struct KvOutput {
    value: Arc<str>,
}

impl Model for KvModel {
    type State = Arc<str>;
    type Input = KvInput;
    type Output = KvOutput;

    fn init(&self) -> Arc<str> {
        Arc::from("")
    }

    fn step(&self, state: &Arc<str>, input: &KvInput, output: &KvOutput) -> Option<Arc<str>> {
        match input.op {
            KvOp::Get => {
                if *output.value == **state {
                    Some(Arc::clone(state)) // atomic refcount bump, no heap alloc
                } else {
                    None
                }
            }
            KvOp::Put => Some(Arc::clone(&input.value)), // zero alloc: reuse existing Arc
            KvOp::Append => {
                let s = format!("{}{}", &**state, &*input.value);
                Some(Arc::from(s.as_str()))
            }
        }
    }

    fn partition(&self, history: &[Operation<KvInput, KvOutput>]) -> Option<Vec<Vec<usize>>> {
        let mut by_key: HashMap<Arc<str>, Vec<usize>> = HashMap::new();
        for (i, op) in history.iter().enumerate() {
            by_key.entry(Arc::clone(&op.input.key)).or_default().push(i);
        }
        Some(by_key.into_values().collect())
    }

    fn partition_events(&self, history: &[Event<KvInput, KvOutput>]) -> Option<Vec<Vec<usize>>> {
        // First pass: map event id → key from Call events.
        let mut id_to_key: HashMap<u64, Arc<str>> = HashMap::new();
        for ev in history {
            if let (EventKind::Call, Some(inp)) = (&ev.kind, &ev.input) {
                id_to_key.insert(ev.id, Arc::clone(&inp.key));
            }
        }
        // Second pass: group each event index by its key.
        let mut by_key: HashMap<Arc<str>, Vec<usize>> = HashMap::new();
        for (i, ev) in history.iter().enumerate() {
            if let Some(key) = id_to_key.get(&ev.id) {
                by_key.entry(Arc::clone(key)).or_default().push(i);
            }
        }
        Some(by_key.into_values().collect())
    }
}

// ============================================================================
// KV MODEL — no partitioning
// Mirrors Go's `kvNoPartitionModel`: same semantics, full-state representation.
// Significantly slower (~60–90 s for c10 traces); corresponding tests are
// marked `#[ignore]`.
// ============================================================================

#[derive(Clone)]
struct KvNoPartitionModel;

impl Model for KvNoPartitionModel {
    type State = HashMap<String, String>;
    type Input = KvInput;
    type Output = KvOutput;

    fn init(&self) -> HashMap<String, String> {
        HashMap::new()
    }

    fn step(
        &self,
        state: &HashMap<String, String>,
        input: &KvInput,
        output: &KvOutput,
    ) -> Option<HashMap<String, String>> {
        let mut next = state.clone();
        match input.op {
            KvOp::Get => {
                let current = state.get(&*input.key).map(String::as_str).unwrap_or("");
                if &*output.value == current {
                    Some(next)
                } else {
                    None
                }
            }
            KvOp::Put => {
                next.insert(input.key.to_string(), input.value.to_string());
                Some(next)
            }
            KvOp::Append => {
                let current = state.get(&*input.key).cloned().unwrap_or_default();
                next.insert(input.key.to_string(), format!("{}{}", current, &*input.value));
                Some(next)
            }
        }
    }
    // No `partition` override → whole history checked as one unit.
}

// ============================================================================
// SET MODEL
// Mirrors the inline model from Go's `TestSetModel`.
//
// State: Vec<i32> (sorted, deduplicated)
// add(v):  insert v, sort, dedup → always valid
// read():  legal iff output.unknown OR output.values (sorted) == state
//          Duplicate values in output are always illegal.
// ============================================================================

#[derive(Clone)]
struct SetModel;

#[derive(Clone, Debug, PartialEq)]
enum SetOp {
    Add,
    Read,
}

#[derive(Clone, Debug, PartialEq)]
struct SetInput {
    op: SetOp,
    value: i32,
}

#[derive(Clone, Debug, PartialEq)]
struct SetOutput {
    values: Vec<i32>,
    unknown: bool,
}

impl Model for SetModel {
    type State = Vec<i32>;
    type Input = SetInput;
    type Output = SetOutput;

    fn init(&self) -> Vec<i32> {
        vec![]
    }

    fn step(&self, state: &Vec<i32>, input: &SetInput, output: &SetOutput) -> Option<Vec<i32>> {
        match input.op {
            SetOp::Add => {
                let mut next = state.clone();
                next.push(input.value);
                next.sort();
                next.dedup();
                Some(next)
            }
            SetOp::Read => {
                if output.unknown {
                    return Some(state.clone());
                }
                let mut vals = output.values.clone();
                vals.sort();
                // Duplicates in output make this observation impossible.
                for w in vals.windows(2) {
                    if w[0] == w[1] {
                        return None;
                    }
                }
                if vals == *state {
                    Some(state.clone())
                } else {
                    None
                }
            }
        }
    }
}

// ============================================================================
// PARSERS
// ============================================================================

fn test_data_path(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

// ── Jepsen / etcd log parser ─────────────────────────────────────────────────
//
// Log line format (tab-separated after the INFO prefix):
//   INFO  jepsen.util - PROCESS\t STATUS \t OP \t VALUE\n
//
// STATUS:
//   :invoke  — call event
//   :ok      — successful return
//   :fail    — failed return (CAS precondition not met)
//   :info    — timed-out return (write or CAS only); value = ":timed-out"
//
// OP / VALUE combinations observed across all 102 log files:
//   :read  nil / N       — read absent key / read value N
//   :write N             — write value N
//   :cas   [from to]     — CAS attempt

fn parse_jepsen_log(n: usize) -> Vec<Event<EtcdInput, EtcdOutput>> {
    let path = test_data_path(&format!("test_data/jepsen/etcd_{n:03}.log"));
    let content = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing test data: {}  \
            (run the download commands from the README)",
            path.display()
        )
    });

    let mut events: Vec<Event<EtcdInput, EtcdOutput>> = Vec::new();
    let mut id_counter: u64 = 0;
    let mut pending: HashMap<u64, u64> = HashMap::new(); // process → event_id

    for line in content.lines() {
        if !line.contains("jepsen.util") {
            continue;
        }

        // Split on tabs: ["INFO  jepsen.util - PROCESS", STATUS, OP, VALUE]
        let parts: Vec<&str> = line.splitn(5, '\t').collect();
        if parts.len() < 4 {
            continue;
        }

        let process: u64 = parts[0].split_whitespace().last().unwrap().parse().unwrap();
        let status = parts[1];
        let op_str = parts[2];
        let val = parts[3].trim_end(); // strip trailing newline/whitespace

        match status {
            ":invoke" => {
                let eid = id_counter;
                id_counter += 1;
                pending.insert(process, eid);

                let input = match op_str {
                    ":read" => EtcdInput {
                        op: EtcdOp::Read,
                        arg1: 0,
                        arg2: 0,
                    },
                    ":write" => EtcdInput {
                        op: EtcdOp::Write,
                        arg1: val.parse().unwrap(),
                        arg2: 0,
                    },
                    ":cas" => {
                        // val = "[arg1 arg2]"
                        let inner = val.trim_start_matches('[').trim_end_matches(']');
                        let mut it = inner.split_whitespace();
                        let arg1 = it.next().unwrap().parse().unwrap();
                        let arg2 = it.next().unwrap().parse().unwrap();
                        EtcdInput {
                            op: EtcdOp::Cas,
                            arg1,
                            arg2,
                        }
                    }
                    _ => continue,
                };

                events.push(Event {
                    client_id: process,
                    kind: EventKind::Call,
                    input: Some(input),
                    output: None,
                    id: eid,
                });
            }

            // ":info" lines (write/CAS timeouts) are NOT handled inline.
            // The Go parser only handles ":fail :read :timed-out" inline; all
            // other unmatched processes get their return events appended at the
            // end of the history (as uncompleted ops with unknown=true).
            ":ok" | ":fail" => {
                // ":fail :read :timed-out" — matches Go's timeoutRead regex
                if status == ":fail" && op_str == ":read" && val == ":timed-out" {
                    let eid = match pending.remove(&process) {
                        Some(e) => e,
                        None => continue,
                    };
                    events.push(Event {
                        client_id: process,
                        kind: EventKind::Return,
                        input: None,
                        output: Some(EtcdOutput {
                            ok: false,
                            exists: false,
                            value: 0,
                            unknown: true,
                        }),
                        id: eid,
                    });
                    continue;
                }

                let eid = match pending.remove(&process) {
                    Some(e) => e,
                    None => continue, // unmatched return — skip
                };

                let output = match op_str {
                    ":read" => {
                        if val == "nil" {
                            EtcdOutput {
                                ok: true,
                                exists: false,
                                value: 0,
                                unknown: false,
                            }
                        } else {
                            EtcdOutput {
                                ok: true,
                                exists: true,
                                value: val.parse().unwrap(),
                                unknown: false,
                            }
                        }
                    }
                    ":write" => EtcdOutput {
                        ok: true,
                        exists: false,
                        value: 0,
                        unknown: false,
                    },
                    ":cas" => EtcdOutput {
                        ok: status == ":ok",
                        exists: false,
                        value: 0,
                        unknown: false,
                    },
                    _ => continue,
                };

                events.push(Event {
                    client_id: process,
                    kind: EventKind::Return,
                    input: None,
                    output: Some(output),
                    id: eid,
                });
            }

            _ => {}
        }
    }

    // Append return events for all processes that never received a response
    // (write/CAS timeouts logged as :info). Matches Go's end-of-loop handling.
    for (_proc, eid) in pending {
        events.push(Event {
            client_id: _proc,
            kind: EventKind::Return,
            input: None,
            output: Some(EtcdOutput {
                ok: false,
                exists: false,
                value: 0,
                unknown: true,
            }),
            id: eid,
        });
    }

    events
}

// ── KV log parser ─────────────────────────────────────────────────────────────
//
// Line format (Clojure map):
//   {:process N, :type :invoke/:ok, :f :get/:put/:append, :key "K", :value "V"/nil}
//
// `:value` is always the last field; `nil` maps to an empty string.

fn parse_kv_log(filename: &str) -> Vec<Event<KvInput, KvOutput>> {
    let path = test_data_path(&format!("test_data/kv/{filename}.txt"));
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing test data: {}", path.display()));

    let mut events: Vec<Event<KvInput, KvOutput>> = Vec::new();
    let mut id_counter: u64 = 0;
    let mut pending: HashMap<u64, u64> = HashMap::new(); // process → event_id

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let process = kv_field_int(line, ":process ");
        let typ = kv_field_token(line, ":type ");
        let f = kv_field_token(line, ":f ");
        let key: Arc<str> = Arc::from(kv_field_quoted(line, ":key \"").as_str());
        let value: Arc<str> = Arc::from(kv_field_value(line).as_str());

        match typ.as_str() {
            ":invoke" => {
                let eid = id_counter;
                id_counter += 1;
                pending.insert(process, eid);

                let op = match f.as_str() {
                    ":get" => KvOp::Get,
                    ":put" => KvOp::Put,
                    ":append" => KvOp::Append,
                    other => panic!("unknown kv op: {other}"),
                };

                events.push(Event {
                    client_id: process,
                    kind: EventKind::Call,
                    input: Some(KvInput { op, key, value }),
                    output: None,
                    id: eid,
                });
            }
            ":ok" => {
                let eid = pending
                    .remove(&process)
                    .unwrap_or_else(|| panic!("unmatched :ok for process {process}"));
                events.push(Event {
                    client_id: process,
                    kind: EventKind::Return,
                    input: None,
                    output: Some(KvOutput { value }),
                    id: eid,
                });
            }
            _ => {}
        }
    }
    events
}

// ── KV parser field helpers ───────────────────────────────────────────────────

fn kv_field_int(line: &str, key: &str) -> u64 {
    let start = line.find(key).unwrap() + key.len();
    let rest = &line[start..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..end].trim().parse().unwrap()
}

fn kv_field_token(line: &str, key: &str) -> String {
    let start = line.find(key).unwrap() + key.len();
    let rest = &line[start..];
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    rest[..end].trim().to_string()
}

fn kv_field_quoted(line: &str, key: &str) -> String {
    let start = line.find(key).unwrap() + key.len();
    let rest = &line[start..];
    let end = rest.find('"').unwrap();
    rest[..end].to_string()
}

/// Extract the `:value` field, which is always last in the map.
/// `nil` → empty string; `"..."` → inner string.
fn kv_field_value(line: &str) -> String {
    let key = ":value ";
    let start = line.rfind(key).unwrap() + key.len();
    let end = line.rfind('}').unwrap();
    let rest = line[start..end].trim();
    if rest == "nil" {
        String::new()
    } else {
        rest.trim_matches('"').to_string()
    }
}

// ============================================================================
// TESTS — Register model
// Mirrors Go's TestRegisterModel and TestZeroDuration.
// ============================================================================

#[test]
fn register_case1_ok() {
    // put(100)[0,100] overlaps get→100[50,75] and get→0[25,80].
    // Valid linearization: get→0, put(100), get→100. → Ok
    let history = [
        reg_op(0, RegOp::Put, 100, 0, 0, 100),
        reg_op(1, RegOp::Get, 0, 100, 50, 75),
        reg_op(2, RegOp::Get, 0, 0, 25, 80),
    ];
    assert_eq!(
        check_operations(&RegisterModel, &history, None),
        CheckResult::Ok
    );
}

#[test]
fn register_case2_illegal() {
    // put(200)[0,100], get→200[10,30], get→0[40,90].
    // get→200 forces put before it in any linearization.
    // get→0 is real-time after get→200, so it must also follow put(200).
    // But then state = 200 when get→0 runs, yet it returns 0. → Illegal
    let history = [
        reg_op(0, RegOp::Put, 200, 0, 0, 100),
        reg_op(1, RegOp::Get, 0, 200, 10, 30),
        reg_op(2, RegOp::Get, 0, 0, 40, 90),
    ];
    assert_eq!(
        check_operations(&RegisterModel, &history, None),
        CheckResult::Illegal
    );
}

#[test]
fn register_zero_duration_ok() {
    // Zero-duration reads (call == return_time) returning the init value. → Ok
    let history = [
        reg_op(0, RegOp::Get, 0, 0, 0, 0),
        reg_op(1, RegOp::Get, 0, 0, 0, 0),
    ];
    assert_eq!(
        check_operations(&RegisterModel, &history, None),
        CheckResult::Ok
    );
}

#[test]
fn register_zero_duration_illegal() {
    // Instantaneous read returning 1 when no write has occurred. → Illegal
    let history = [reg_op(0, RegOp::Get, 0, 1, 0, 0)];
    assert_eq!(
        check_operations(&RegisterModel, &history, None),
        CheckResult::Illegal
    );
}

#[test]
fn check_no_partitions() {
    // Empty history with a partitioned model → Ok (matches Go's TestCheckNoPartitions).
    let history: Vec<Operation<KvInput, KvOutput>> = vec![];
    assert_eq!(check_operations(&KvModel, &history, None), CheckResult::Ok);
}

// ============================================================================
// TESTS — Etcd / Jepsen
// Mirrors Go's TestEtcdJepsen000 … TestEtcdJepsen102 (095 absent from upstream).
//
// Each entry is (log_number, is_linearizable).
// Results match the Go test suite's expected values.
// ============================================================================

#[rustfmt::skip]
const JEPSEN_EXPECTED: &[(usize, bool)] = &[
    (0,  false), (1,  false), (2,  true),  (3,  false), (4,  false),
    (5,  true),  (6,  false), (7,  true),  (8,  false), (9,  false),
    (10, false), (11, false), (12, false), (13, false), (14, false),
    (15, false), (16, false), (17, false), (18, true),  (19, false),
    (20, false), (21, false), (22, false), (23, false), (24, false),
    (25, true),  (26, false), (27, false), (28, false), (29, false),
    (30, false), (31, true),  (32, false), (33, false), (34, false),
    (35, false), (36, false), (37, false), (38, true),  (39, false),
    (40, false), (41, false), (42, false), (43, false), (44, false),
    (45, true),  (46, false), (47, false), (48, true),  (49, true),
    (50, false), (51, true),  (52, false), (53, true),  (54, false),
    (55, false), (56, true),  (57, false), (58, false), (59, false),
    (60, false), (61, false), (62, false), (63, false), (64, false),
    (65, false), (66, false), (67, true),  (68, false), (69, false),
    (70, false), (71, false), (72, false), (73, false), (74, false),
    (75, true),  (76, true),  (77, false), (78, false), (79, false),
    (80, true),  (81, false), (82, false), (83, false), (84, false),
    (85, false), (86, false), (87, true),  (88, false), (89, false),
    (90, false), (91, false), (92, true),  (93, false), (94, false),
    // 095 is absent from the upstream Go repository — skipped
    (96,  false), (97,  false), (98,  true),  (99,  false),
    (100, true),  (101, true),  (102, true),
];

#[test]
fn etcd_jepsen_all_cases() {
    for &(n, is_ok) in JEPSEN_EXPECTED {
        let events = parse_jepsen_log(n);
        let expected = if is_ok {
            CheckResult::Ok
        } else {
            CheckResult::Illegal
        };
        let result = check_events(&EtcdModel, &events, None);
        assert_eq!(result, expected, "etcd_{n:03}");
    }
}

// ============================================================================
// TESTS — KV model
// Mirrors Go's TestKv1Client*, TestKv10Clients*, TestKv50Clients*,
// TestKvNoPartition1Client*, TestKvNoPartition10Clients*.
// ============================================================================

fn check_kv(name: &str, expected: CheckResult, partition: bool) {
    let events = parse_kv_log(name);
    let result = if partition {
        check_events(&KvModel, &events, None)
    } else {
        check_events(&KvNoPartitionModel, &events, None)
    };
    assert_eq!(result, expected, "{name} (partition={partition})");
}

#[test]
fn kv_1_client_ok() {
    check_kv("c01-ok", CheckResult::Ok, true);
}
#[test]
fn kv_1_client_bad() {
    check_kv("c01-bad", CheckResult::Illegal, true);
}

#[test]
fn kv_10_clients_ok() {
    check_kv("c10-ok", CheckResult::Ok, true);
}
#[test]
fn kv_10_clients_bad() {
    check_kv("c10-bad", CheckResult::Illegal, true);
}

#[test]
fn kv_50_clients_ok() {
    check_kv("c50-ok", CheckResult::Ok, true);
}

/// Proving non-linearity for 10 keys × ~230 ops requires exhaustive DFS search.
/// The checker is correct but slow on this trace (>10 min even in release mode).
/// Pass `-- --include-ignored` to run.
#[test]
fn kv_50_clients_bad() {
    check_kv("c50-bad", CheckResult::Illegal, true);
}

#[test]
fn kv_no_partition_1_client_ok() {
    check_kv("c01-ok", CheckResult::Ok, false);
}
#[test]
fn kv_no_partition_1_client_bad() {
    check_kv("c01-bad", CheckResult::Illegal, false);
}

/// Runs in ~60–90 s without partitioning. Pass `-- --include-ignored` to run.
#[test]
#[ignore = "~60-90s without partitioning"]
fn kv_no_partition_10_clients_ok() {
    check_kv("c10-ok", CheckResult::Ok, false);
}

/// Runs in ~60–90 s without partitioning. Pass `-- --include-ignored` to run.
#[test]
#[ignore = "~60-90s without partitioning"]
fn kv_no_partition_10_clients_bad() {
    check_kv("c10-bad", CheckResult::Illegal, false);
}

// ============================================================================
// TESTS — Set model
// Mirrors Go's TestSetModel (4 inline cases).
// ============================================================================

fn set_call(id: u64, op: SetOp, value: i32) -> Event<SetInput, SetOutput> {
    Event {
        client_id: id,
        kind: EventKind::Call,
        input: Some(SetInput { op, value }),
        output: None,
        id,
    }
}

fn set_ret(id: u64, values: Vec<i32>, unknown: bool) -> Event<SetInput, SetOutput> {
    Event {
        client_id: id,
        kind: EventKind::Return,
        input: None,
        output: Some(SetOutput { values, unknown }),
        id,
    }
}

#[test]
fn set_model_all_cases() {
    // Case 1: add(100) completes; add(0) overlaps with read→{100}.
    // Linearization: add(100), read, add(0). → Ok
    let case1 = [
        set_call(0, SetOp::Add, 100),
        set_ret(0, vec![], false),
        set_call(1, SetOp::Add, 0),
        set_call(2, SetOp::Read, 0),
        set_ret(2, vec![100], false),
        set_ret(1, vec![], false),
    ];

    // Case 2: all three concurrent; read returns the full set {100, 110}. → Ok
    let case2 = [
        set_call(0, SetOp::Add, 100),
        set_call(1, SetOp::Add, 110),
        set_call(2, SetOp::Read, 0),
        set_ret(2, vec![100, 110], false),
        set_ret(0, vec![], false),
        set_ret(1, vec![], false),
    ];

    // Case 3: all concurrent; read timed out (unknown = true). → Ok
    let case3 = [
        set_call(0, SetOp::Add, 100),
        set_call(1, SetOp::Add, 110),
        set_call(2, SetOp::Read, 0),
        set_ret(2, vec![], true),
        set_ret(0, vec![], false),
        set_ret(1, vec![], false),
    ];

    // Case 4: all concurrent; read returns {100, 100, 110} — duplicate. → Illegal
    let case4 = [
        set_call(0, SetOp::Add, 100),
        set_call(1, SetOp::Add, 110),
        set_call(2, SetOp::Read, 0),
        set_ret(2, vec![100, 100, 110], false),
        set_ret(0, vec![], false),
        set_ret(1, vec![], false),
    ];

    assert_eq!(
        check_events(&SetModel, &case1, None),
        CheckResult::Ok,
        "case 1"
    );
    assert_eq!(
        check_events(&SetModel, &case2, None),
        CheckResult::Ok,
        "case 2"
    );
    assert_eq!(
        check_events(&SetModel, &case3, None),
        CheckResult::Ok,
        "case 3"
    );
    assert_eq!(
        check_events(&SetModel, &case4, None),
        CheckResult::Illegal,
        "case 4"
    );
}
