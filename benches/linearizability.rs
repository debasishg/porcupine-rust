//! Criterion benchmarks: porcupine-rust linearizability checker.
//!
//! Benchmark groups:
//!   etcd_sequential  — 1 rayon thread; apples-to-apples with Go's single-threaded default
//!   etcd_parallel    — all rayon threads; shows the parallelism dividend
//!   kv_partitioned   — KV model with per-key partitioning (c10 traces)
//!
//! Run:
//!   cargo bench --bench linearizability                  # all groups
//!   cargo bench --bench linearizability -- etcd_seq      # one group
//!   cargo bench --bench linearizability -- --test        # dry-run (no timing)

use criterion::{BenchmarkGroup, BenchmarkId, Criterion, criterion_group, criterion_main};
use criterion::measurement::WallTime;
use porcupine::checker::check_events;
use porcupine::{Event, EventKind, Model};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

// ============================================================================
// ETCD MODEL  (mirrors Go's etcdModel and tests/go_compat.rs::EtcdModel)
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
        match input.op {
            EtcdOp::Read => {
                let ok = match state {
                    None => !output.exists || output.unknown,
                    Some(v) => (output.exists && output.value == *v) || output.unknown,
                };
                if ok { Some(*state) } else { None }
            }
            EtcdOp::Write => Some(Some(input.arg1)),
            EtcdOp::Cas => {
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
                let ok = (st_matches && output.ok)
                    || (!st_matches && !output.ok)
                    || output.unknown;
                if ok { Some(next_state) } else { None }
            }
        }
    }
}

// ============================================================================
// KV MODEL  (mirrors Go's kvModel and tests/go_compat.rs::KvModel)
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
                if &*output.value == &**state {
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

    fn partition_events(&self, history: &[Event<KvInput, KvOutput>]) -> Option<Vec<Vec<usize>>> {
        let mut id_to_key: HashMap<u64, Arc<str>> = HashMap::new();
        for ev in history {
            if let (EventKind::Call, Some(inp)) = (&ev.kind, &ev.input) {
                id_to_key.insert(ev.id, Arc::clone(&inp.key));
            }
        }
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
// PARSERS
// ============================================================================

fn test_data_path(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Parse one Jepsen etcd log file into events.
/// Mirrors the parser in tests/go_compat.rs.
fn parse_jepsen_log(n: usize) -> Vec<Event<EtcdInput, EtcdOutput>> {
    let path = test_data_path(&format!("test_data/jepsen/etcd_{n:03}.log"));
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing test data: {}", path.display()));

    let mut events: Vec<Event<EtcdInput, EtcdOutput>> = Vec::new();
    let mut id_counter: u64 = 0;
    let mut pending: HashMap<u64, u64> = HashMap::new();

    for line in content.lines() {
        if !line.contains("jepsen.util") {
            continue;
        }
        let parts: Vec<&str> = line.splitn(5, '\t').collect();
        if parts.len() < 4 {
            continue;
        }
        let process: u64 = parts[0].split_whitespace().last().unwrap().parse().unwrap();
        let status = parts[1];
        let op_str = parts[2];
        let val = parts[3].trim_end();

        match status {
            ":invoke" => {
                let eid = id_counter;
                id_counter += 1;
                pending.insert(process, eid);

                let input = match op_str {
                    ":read" => EtcdInput { op: EtcdOp::Read, arg1: 0, arg2: 0 },
                    ":write" => EtcdInput { op: EtcdOp::Write, arg1: val.parse().unwrap(), arg2: 0 },
                    ":cas" => {
                        let inner = val.trim_start_matches('[').trim_end_matches(']');
                        let mut it = inner.split_whitespace();
                        let arg1 = it.next().unwrap().parse().unwrap();
                        let arg2 = it.next().unwrap().parse().unwrap();
                        EtcdInput { op: EtcdOp::Cas, arg1, arg2 }
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

            ":ok" | ":fail" => {
                if status == ":fail" && op_str == ":read" && val == ":timed-out" {
                    let eid = match pending.remove(&process) {
                        Some(e) => e,
                        None => continue,
                    };
                    events.push(Event {
                        client_id: process,
                        kind: EventKind::Return,
                        input: None,
                        output: Some(EtcdOutput { ok: false, exists: false, value: 0, unknown: true }),
                        id: eid,
                    });
                    continue;
                }

                let eid = match pending.remove(&process) {
                    Some(e) => e,
                    None => continue,
                };

                let output = match op_str {
                    ":read" => {
                        if val == "nil" {
                            EtcdOutput { ok: true, exists: false, value: 0, unknown: false }
                        } else {
                            EtcdOutput {
                                ok: true,
                                exists: true,
                                value: val.parse().unwrap(),
                                unknown: false,
                            }
                        }
                    }
                    ":write" => EtcdOutput { ok: true, exists: false, value: 0, unknown: false },
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

    for (_proc, eid) in pending {
        events.push(Event {
            client_id: _proc,
            kind: EventKind::Return,
            input: None,
            output: Some(EtcdOutput { ok: false, exists: false, value: 0, unknown: true }),
            id: eid,
        });
    }

    events
}

/// Parse all 102 Jepsen etcd log files (skips missing files gracefully).
fn load_all_jepsen_logs() -> Vec<Vec<Event<EtcdInput, EtcdOutput>>> {
    (0..=102)
        .filter_map(|n| {
            let path = test_data_path(&format!("test_data/jepsen/etcd_{n:03}.log"));
            if path.exists() { Some(parse_jepsen_log(n)) } else { None }
        })
        .collect()
}

/// Parse a KV trace file.
fn parse_kv_log(filename: &str) -> Vec<Event<KvInput, KvOutput>> {
    let path = test_data_path(&format!("test_data/kv/{filename}.txt"));
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing test data: {}", path.display()));

    let mut events: Vec<Event<KvInput, KvOutput>> = Vec::new();
    let mut id_counter: u64 = 0;
    let mut pending: HashMap<u64, u64> = HashMap::new();

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

fn kv_field_value(line: &str) -> String {
    let key = ":value ";
    let start = line.rfind(key).unwrap() + key.len();
    let end = line.rfind('}').unwrap();
    let rest = line[start..end].trim();
    if rest == "nil" { String::new() } else { rest.trim_matches('"').to_string() }
}

// ============================================================================
// BENCHMARK GROUPS
// ============================================================================

/// Group A: single-threaded etcd — apples-to-apples with Go.
///
/// Uses a dedicated rayon ThreadPool with 1 thread so that check_events
/// runs sequentially, matching Go's default single-threaded behaviour.
fn bench_etcd_sequential(c: &mut Criterion) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();

    let single_history = parse_jepsen_log(0);
    let all_histories = load_all_jepsen_logs();
    let n_loaded = all_histories.len();

    let mut group = c.benchmark_group("etcd_sequential");
    group.measurement_time(Duration::from_secs(10));

    group.bench_function("single_file", |b| {
        b.iter(|| {
            pool.install(|| {
                check_events(&EtcdModel, &single_history, None)
            })
        });
    });

    group.bench_function(
        BenchmarkId::new("all_files", n_loaded),
        |b| {
            b.iter(|| {
                pool.install(|| {
                    for h in &all_histories {
                        let _ = check_events(&EtcdModel, h, None);
                    }
                })
            });
        },
    );

    group.finish();
}

/// Group B: multi-threaded etcd — shows the rayon parallelism dividend.
///
/// Uses the global rayon thread pool (all available cores).
fn bench_etcd_parallel(c: &mut Criterion) {
    let single_history = parse_jepsen_log(0);
    let all_histories = load_all_jepsen_logs();
    let n_loaded = all_histories.len();

    let mut group = c.benchmark_group("etcd_parallel");
    group.measurement_time(Duration::from_secs(10));

    group.bench_function("single_file", |b| {
        b.iter(|| check_events(&EtcdModel, &single_history, None));
    });

    group.bench_function(
        BenchmarkId::new("all_files", n_loaded),
        |b| {
            b.iter(|| {
                all_histories.par_iter().for_each(|h| {
                    let _ = check_events(&EtcdModel, h, None);
                });
            });
        },
    );

    group.finish();
}

/// Group C: KV model with per-key partitioning.
///
/// c10 traces: 10 concurrent clients — realistic workload without the
/// multi-hour runtime of c50.  Both single-thread and parallel variants
/// so we get the same apples-to-apples split as the etcd groups.
fn bench_kv_partitioned(c: &mut Criterion) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();

    let c10_ok  = parse_kv_log("c10-ok");
    let c10_bad = parse_kv_log("c10-bad");

    let mut group: BenchmarkGroup<WallTime> = c.benchmark_group("kv_partitioned");
    group.measurement_time(Duration::from_secs(30));
    group.sample_size(20); // KV traces are heavier; fewer samples keep bench time reasonable

    // --- sequential (1 thread) ---
    group.bench_function("c10_ok_seq", |b| {
        b.iter(|| pool.install(|| check_events(&KvModel, &c10_ok, None)));
    });
    group.bench_function("c10_bad_seq", |b| {
        b.iter(|| pool.install(|| check_events(&KvModel, &c10_bad, None)));
    });

    // --- parallel (all threads) ---
    group.bench_function("c10_ok_par", |b| {
        b.iter(|| check_events(&KvModel, &c10_ok, None));
    });
    group.bench_function("c10_bad_par", |b| {
        b.iter(|| check_events(&KvModel, &c10_bad, None));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_etcd_sequential,
    bench_etcd_parallel,
    bench_kv_partitioned,
);
criterion_main!(benches);
