/// CLI tool: check an S2 stream history for linearizability.
///
/// Reads a JSONL file produced by `collect-history` from s2-streamstore/s2-verification
/// and runs the porcupine-rust checker against the S2 stream model.
///
/// USAGE:
///   cargo run --example s2_checker -- <path/to/records.jsonl> [timeout_secs]
///
/// EXIT CODES:
///   0  — history is linearizable
///   1  — history is NOT linearizable, or timed out
use porcupine::{
    checker::check_events,
    model::{NondeterministicModel, PowerSetModel},
    types::{CheckResult, Event, EventKind},
};
use serde_json::Value;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Types (duplicated from tests/s2_model.rs — kept local to avoid a public API)
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Debug)]
struct S2StreamState {
    tail: u32,
    xxh3: u64,
    fencing_token: Option<String>,
}

#[derive(Clone, PartialEq, Debug)]
enum S2Input {
    Append {
        num_records: u32,
        xxh3: u64,
        set_fencing_token: Option<String>,
        fencing_token: Option<String>,
        match_seq_num: Option<u32>,
    },
    Read,
    CheckTail,
}

#[derive(Clone, PartialEq, Debug)]
enum S2Output {
    AppendSuccess { tail: u32 },
    AppendDefiniteFailure,
    AppendIndefiniteFailure,
    ReadSuccess { tail: u32, xxh3: u64 },
    ReadFailure,
    CheckTailSuccess { tail: u32 },
    CheckTailFailure,
}

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
                let optimistic_token = set_fencing_token
                    .clone()
                    .or_else(|| state.fencing_token.clone());
                let optimistic = S2StreamState {
                    tail: state.tail + num_records,
                    xxh3: *xxh3,
                    fencing_token: optimistic_token,
                };
                match output {
                    S2Output::AppendDefiniteFailure => vec![state.clone()],
                    S2Output::AppendIndefiniteFailure => {
                        if let Some(bt) = fencing_token {
                            if let Some(ft) = &state.fencing_token {
                                if bt != ft {
                                    return vec![state.clone()];
                                }
                            }
                        }
                        if let Some(msn) = match_seq_num {
                            if *msn != state.tail {
                                return vec![state.clone()];
                            }
                        }
                        vec![optimistic, state.clone()]
                    }
                    S2Output::AppendSuccess { tail } => {
                        if let Some(bt) = fencing_token {
                            match &state.fencing_token {
                                None => return vec![],
                                Some(ft) if ft != bt => return vec![],
                                _ => {}
                            }
                        }
                        if let Some(msn) = match_seq_num {
                            if *msn != state.tail {
                                return vec![];
                            }
                        }
                        if *tail != optimistic.tail {
                            return vec![];
                        }
                        vec![optimistic]
                    }
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
// ---------------------------------------------------------------------------

fn parse_start(v: &Value) -> S2Input {
    if let Some(s) = v.as_str() {
        return match s {
            "Read" => S2Input::Read,
            "CheckTail" => S2Input::CheckTail,
            other => panic!("unknown Start string: {other}"),
        };
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
        return S2Output::AppendSuccess {
            tail: inner["tail"].as_u64().unwrap() as u32,
        };
    }
    if let Some(inner) = v.get("ReadSuccess") {
        return S2Output::ReadSuccess {
            tail: inner["tail"].as_u64().unwrap() as u32,
            xxh3: inner["xxh3"].as_u64().unwrap(),
        };
    }
    if let Some(inner) = v.get("CheckTailSuccess") {
        return S2Output::CheckTailSuccess {
            tail: inner["tail"].as_u64().unwrap() as u32,
        };
    }
    panic!("unrecognised Finish event: {v}");
}

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
            events.push(Event {
                client_id,
                kind: EventKind::Call,
                input: Some(parse_start(start_val)),
                output: None,
                id: op_id,
            });
        } else if let Some(finish_val) = event_obj.get("Finish") {
            events.push(Event {
                client_id,
                kind: EventKind::Return,
                input: None,
                output: Some(parse_finish(finish_val)),
                id: op_id,
            });
        } else {
            panic!("event has neither Start nor Finish: {event_obj}");
        }
    }
    events
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("USAGE: s2_checker <path/to/records.jsonl> [timeout_secs]");
        std::process::exit(1);
    }

    let path = &args[1];
    let timeout: Option<Duration> = args.get(2).map(|s| {
        let secs: u64 = s.parse().unwrap_or_else(|_| {
            eprintln!("invalid timeout: {s}");
            std::process::exit(1);
        });
        Duration::from_secs(secs)
    });

    let content = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("failed to read {path}: {e}");
        std::process::exit(1);
    });

    let events = parse_s2_jsonl(&content);
    eprintln!("parsed {} events from {path}", events.len());

    let result = check_events(&PowerSetModel(S2StreamModel), &events, timeout);

    match result {
        CheckResult::Ok => {
            println!("PASS: history is linearizable");
            std::process::exit(0);
        }
        CheckResult::Illegal => {
            println!("FAIL: history is NOT linearizable");
            std::process::exit(1);
        }
        CheckResult::Unknown => {
            println!("UNKNOWN: check timed out");
            std::process::exit(1);
        }
    }
}
