/// Integration tests for the S2 stream-storage linearizability model.
///
/// S2 is an append-only log service.  Streams have a monotonically-increasing
/// tail, an xxh3 hash of the most-recently-appended batch, and an optional
/// fencing token used to guard concurrent writers.
///
/// The model is nondeterministic because an `AppendIndefiniteFailure` may or
/// may not have become durable.  We wrap it in `PowerSetModel` to drive the
/// standard `check_events` entry-point.
///
/// The five unit tests are direct ports of the Go test suite in
/// `golang/s2-porcupine/main_test.go` from s2-streamstore/s2-verification.
use porcupine::{
    checker::check_events,
    model::{NondeterministicModel, PowerSetModel},
    types::{CheckResult, Event, EventKind},
};
use serde_json::Value;

// ---------------------------------------------------------------------------
// State and I/O types
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Debug)]
struct S2StreamState {
    tail: u32,
    xxh3: u64,
    fencing_token: Option<String>,
}

/// The input side of an S2 operation, captured at invocation time.
#[derive(Clone, PartialEq, Debug)]
enum S2Input {
    Append {
        num_records: u32,
        /// xxh3 hash of the last record in the batch.
        xxh3: u64,
        /// If Some, install this as the new stream-level fencing token on success.
        set_fencing_token: Option<String>,
        /// Guard: the append must be rejected if the stream's current fencing
        /// token does not equal this value.
        fencing_token: Option<String>,
        /// Guard: the append must be rejected if the current tail does not
        /// equal this value.
        match_seq_num: Option<u32>,
    },
    Read,
    CheckTail,
}

/// The output side of an S2 operation, observed at return time.
#[derive(Clone, PartialEq, Debug)]
enum S2Output {
    AppendSuccess { tail: u32 },
    /// The append definitely did not become durable (validation/fencing error).
    AppendDefiniteFailure,
    /// Network or server error — the append may or may not have become durable.
    AppendIndefiniteFailure,
    ReadSuccess { tail: u32, xxh3: u64 },
    ReadFailure,
    CheckTailSuccess { tail: u32 },
    CheckTailFailure,
}

// ---------------------------------------------------------------------------
// NondeterministicModel implementation
// ---------------------------------------------------------------------------

struct S2StreamModel;

impl NondeterministicModel for S2StreamModel {
    type State = S2StreamState;
    type Input = S2Input;
    type Output = S2Output;

    fn init(&self) -> Vec<S2StreamState> {
        vec![S2StreamState {
            tail: 0,
            xxh3: 0,
            fencing_token: None,
        }]
    }

    fn step(
        &self,
        state: &S2StreamState,
        input: &S2Input,
        output: &S2Output,
    ) -> Vec<S2StreamState> {
        match input {
            S2Input::Append {
                num_records,
                xxh3,
                set_fencing_token,
                fencing_token,
                match_seq_num,
            } => {
                // The state that results if the append became durable.
                let optimistic_token = set_fencing_token
                    .clone()
                    .or_else(|| state.fencing_token.clone());
                let optimistic = S2StreamState {
                    tail: state.tail + num_records,
                    xxh3: *xxh3,
                    fencing_token: optimistic_token,
                };

                match output {
                    S2Output::AppendDefiniteFailure => {
                        // Guaranteed not durable.
                        vec![state.clone()]
                    }
                    S2Output::AppendIndefiniteFailure => {
                        // Fencing token mismatch → cannot have become durable.
                        if let Some(bt) = fencing_token
                            && let Some(ft) = &state.fencing_token
                            && bt != ft
                        {
                            return vec![state.clone()];
                        }
                        // matchSeqNum mismatch → cannot have become durable.
                        if let Some(msn) = match_seq_num
                            && *msn != state.tail
                        {
                            return vec![state.clone()];
                        }
                        // Both outcomes are possible.
                        vec![optimistic, state.clone()]
                    }
                    S2Output::AppendSuccess { tail } => {
                        // Durable: validate preconditions.
                        if let Some(bt) = fencing_token {
                            match &state.fencing_token {
                                None => return vec![],
                                Some(ft) if ft != bt => return vec![],
                                _ => {}
                            }
                        }
                        if let Some(msn) = match_seq_num
                            && *msn != state.tail
                        {
                            return vec![];
                        }
                        if *tail != optimistic.tail {
                            return vec![];
                        }
                        vec![optimistic]
                    }
                    // Wrong output kind for an Append input.
                    _ => vec![],
                }
            }

            S2Input::Read => match output {
                S2Output::ReadSuccess { tail, xxh3 } => {
                    if state.xxh3 != *xxh3 {
                        return vec![];
                    }
                    if state.tail == *tail {
                        vec![state.clone()]
                    } else {
                        vec![]
                    }
                }
                S2Output::ReadFailure => vec![state.clone()],
                _ => vec![],
            },

            S2Input::CheckTail => match output {
                S2Output::CheckTailSuccess { tail } => {
                    if state.tail == *tail {
                        vec![state.clone()]
                    } else {
                        vec![]
                    }
                }
                S2Output::CheckTailFailure => vec![state.clone()],
                _ => vec![],
            },
        }
    }
}

// ---------------------------------------------------------------------------
// JSONL parser
//
// Converts the event-log format produced by `collect-history` in
// s2-streamstore/s2-verification into porcupine `Event<S2Input, S2Output>`.
//
// Format (one JSON object per line):
//
//   {"event":{"Start":{"Append":{...}}},"client_id":2,"op_id":4984}
//   {"event":{"Finish":{"AppendSuccess":{"tail":5}}},"client_id":2,"op_id":4984}
//   {"event":{"Finish":"AppendDefiniteFailure"},"client_id":2,"op_id":4984}
//
// Because the "Start"/"Finish" values mix strings and objects we parse via
// `serde_json::Value` and pattern-match manually rather than fight serde's
// untagged enum machinery.
// ---------------------------------------------------------------------------

fn parse_start(v: &Value) -> S2Input {
    if let Some(s) = v.as_str() {
        match s {
            "Read" => return S2Input::Read,
            "CheckTail" => return S2Input::CheckTail,
            other => panic!("unknown Start string: {other}"),
        }
    }
    if let Some(append) = v.get("Append") {
        let num_records = append["num_records"].as_u64().unwrap() as u32;
        let xxh3 = append["last_record_xxh3"].as_u64().unwrap();
        let set_fencing_token = append
            .get("set_fencing_token")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let fencing_token = append
            .get("fencing_token")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let match_seq_num = append
            .get("match_seq_num")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);
        return S2Input::Append {
            num_records,
            xxh3,
            set_fencing_token,
            fencing_token,
            match_seq_num,
        };
    }
    panic!("unrecognised Start event: {v}");
}

fn parse_finish(v: &Value) -> S2Output {
    if let Some(s) = v.as_str() {
        return match s {
            "AppendDefiniteFailure" => S2Output::AppendDefiniteFailure,
            "AppendIndefiniteFailure" => S2Output::AppendIndefiniteFailure,
            "ReadFailure" => S2Output::ReadFailure,
            "CheckTailFailure" => S2Output::CheckTailFailure,
            other => panic!("unknown Finish string: {other}"),
        };
    }
    if let Some(inner) = v.get("AppendSuccess") {
        let tail = inner["tail"].as_u64().unwrap() as u32;
        return S2Output::AppendSuccess { tail };
    }
    if let Some(inner) = v.get("ReadSuccess") {
        let tail = inner["tail"].as_u64().unwrap() as u32;
        let xxh3 = inner["xxh3"].as_u64().unwrap();
        return S2Output::ReadSuccess { tail, xxh3 };
    }
    if let Some(inner) = v.get("CheckTailSuccess") {
        let tail = inner["tail"].as_u64().unwrap() as u32;
        return S2Output::CheckTailSuccess { tail };
    }
    panic!("unrecognised Finish event: {v}");
}

/// Parse a JSONL string (one record per line) into a porcupine event list.
fn parse_s2_jsonl(input: &str) -> Vec<Event<S2Input, S2Output>> {
    let mut events = Vec::new();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("failed to parse line: {e}\nline: {line}"));

        let client_id = record["client_id"].as_u64().unwrap();
        let op_id = record["op_id"].as_u64().unwrap();
        let event_obj = &record["event"];

        if let Some(start_val) = event_obj.get("Start") {
            let input = parse_start(start_val);
            events.push(Event {
                client_id,
                kind: EventKind::Call,
                input: Some(input),
                output: None,
                id: op_id,
            });
        } else if let Some(finish_val) = event_obj.get("Finish") {
            let output = parse_finish(finish_val);
            events.push(Event {
                client_id,
                kind: EventKind::Return,
                input: None,
                output: Some(output),
                id: op_id,
            });
        } else {
            panic!("event has neither Start nor Finish: {event_obj}");
        }
    }
    events
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn call(client_id: u64, op_id: u64, input: S2Input) -> Event<S2Input, S2Output> {
    Event {
        client_id,
        kind: EventKind::Call,
        input: Some(input),
        output: None,
        id: op_id,
    }
}

fn ret(client_id: u64, op_id: u64, output: S2Output) -> Event<S2Input, S2Output> {
    Event {
        client_id,
        kind: EventKind::Return,
        input: None,
        output: Some(output),
        id: op_id,
    }
}

fn append_input(num_records: u32, xxh3: u64) -> S2Input {
    S2Input::Append {
        num_records,
        xxh3,
        set_fencing_token: None,
        fencing_token: None,
        match_seq_num: None,
    }
}

fn check(events: &[Event<S2Input, S2Output>]) -> CheckResult {
    check_events(&PowerSetModel(S2StreamModel), events, None)
}

// ---------------------------------------------------------------------------
// Unit tests — ports of golang/s2-porcupine/main_test.go
// ---------------------------------------------------------------------------

/// Port of TestBasicNoConcurrency.
/// Append(4) → Read → CheckTail, all succeed sequentially.
#[test]
fn basic_no_concurrency() {
    let events = vec![
        call(0, 0, append_input(4, 12345)),
        ret(0, 0, S2Output::AppendSuccess { tail: 4 }),
        call(0, 1, S2Input::Read),
        ret(0, 1, S2Output::ReadSuccess { tail: 4, xxh3: 12345 }),
        call(0, 2, S2Input::CheckTail),
        ret(0, 2, S2Output::CheckTailSuccess { tail: 4 }),
    ];
    assert_eq!(check(&events), CheckResult::Ok);
}

/// Port of TestBasicNoConcurrencyDefiniteFailure1.
/// Append(4) ok → Read ok → CheckTail ok → Append(5) definite-fail → Read still sees tail=4.
#[test]
fn definite_failure_linearizable() {
    let events = vec![
        call(0, 0, append_input(4, 12345)),
        ret(0, 0, S2Output::AppendSuccess { tail: 4 }),
        call(0, 1, S2Input::Read),
        ret(0, 1, S2Output::ReadSuccess { tail: 4, xxh3: 12345 }),
        call(0, 2, S2Input::CheckTail),
        ret(0, 2, S2Output::CheckTailSuccess { tail: 4 }),
        call(0, 3, append_input(5, 67890)),
        ret(0, 3, S2Output::AppendDefiniteFailure),
        call(0, 4, S2Input::Read),
        ret(0, 4, S2Output::ReadSuccess { tail: 4, xxh3: 12345 }),
    ];
    assert_eq!(check(&events), CheckResult::Ok);
}

/// Port of TestBasicNoConcurrencyDefiniteFailure2.
/// Same as above but the final Read returns tail=9 (the failed append's tail) — illegal.
#[test]
fn definite_failure_illegal() {
    let events = vec![
        call(0, 0, append_input(4, 12345)),
        ret(0, 0, S2Output::AppendSuccess { tail: 4 }),
        call(0, 1, S2Input::Read),
        ret(0, 1, S2Output::ReadSuccess { tail: 4, xxh3: 12345 }),
        call(0, 2, S2Input::CheckTail),
        ret(0, 2, S2Output::CheckTailSuccess { tail: 4 }),
        call(0, 3, append_input(5, 67890)),
        ret(0, 3, S2Output::AppendDefiniteFailure),
        // Read returns updated tail despite definite failure — illegal.
        call(0, 4, S2Input::Read),
        ret(0, 4, S2Output::ReadSuccess { tail: 9, xxh3: 67890 }),
    ];
    assert_eq!(check(&events), CheckResult::Illegal);
}

/// Port of TestBasicNoConcurrencyIndefiniteFailure1.
/// Append(5) indefinite-fails, then Read returns the updated tail=9 — still linearizable
/// because an indefinite failure may have become durable.
#[test]
fn indefinite_failure_updated_tail_ok() {
    let events = vec![
        call(0, 0, append_input(4, 12345)),
        ret(0, 0, S2Output::AppendSuccess { tail: 4 }),
        call(0, 1, S2Input::Read),
        ret(0, 1, S2Output::ReadSuccess { tail: 4, xxh3: 12345 }),
        call(0, 2, S2Input::CheckTail),
        ret(0, 2, S2Output::CheckTailSuccess { tail: 4 }),
        call(0, 3, append_input(5, 67890)),
        ret(0, 3, S2Output::AppendIndefiniteFailure),
        call(0, 4, S2Input::Read),
        ret(0, 4, S2Output::ReadSuccess { tail: 9, xxh3: 67890 }),
    ];
    assert_eq!(check(&events), CheckResult::Ok);
}

/// Port of TestBasicNoConcurrencyIndefiniteFailure2.
/// Same but Read returns the original tail=4 — also linearizable
/// (the indefinite failure did not become durable in this execution).
#[test]
fn indefinite_failure_original_tail_ok() {
    let events = vec![
        call(0, 0, append_input(4, 12345)),
        ret(0, 0, S2Output::AppendSuccess { tail: 4 }),
        call(0, 1, S2Input::Read),
        ret(0, 1, S2Output::ReadSuccess { tail: 4, xxh3: 12345 }),
        call(0, 2, S2Input::CheckTail),
        ret(0, 2, S2Output::CheckTailSuccess { tail: 4 }),
        call(0, 3, append_input(5, 67890)),
        ret(0, 3, S2Output::AppendIndefiniteFailure),
        call(0, 4, S2Input::Read),
        ret(0, 4, S2Output::ReadSuccess { tail: 4, xxh3: 12345 }),
    ];
    assert_eq!(check(&events), CheckResult::Ok);
}

// ---------------------------------------------------------------------------
// File-based integration test
//
// Reads a JSONL history produced by `collect-history` from s2-verification.
// Silently skips when the file is absent (no S2 account required for CI).
//
// Usage:
//   S2_HISTORY_FILE=path/to/records.jsonl cargo test --test s2_model check_jsonl_file_if_present
// ---------------------------------------------------------------------------

#[test]
fn check_jsonl_file_if_present() {
    let path = std::env::var("S2_HISTORY_FILE")
        .unwrap_or_else(|_| "test_data/s2_records.jsonl".into());
    if !std::path::Path::new(&path).exists() {
        return;
    }
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let events = parse_s2_jsonl(&content);
    let result = check_events(&PowerSetModel(S2StreamModel), &events, None);
    assert_eq!(
        result,
        CheckResult::Ok,
        "s2 history at {path} is not linearizable"
    );
}
