// Model-based testing: replay Quint ITF traces against the Rust implementation.
//
// This test file is gated behind the `quint-mbt` feature flag.
// It requires the `quint` CLI (≥ 0.31.0) to be installed.
//
// Run:  cargo test --features quint-mbt --test quint_mbt
//
// How it works:
//  1. `quint run tla/Porcupine.qnt` generates ITF execution traces as JSON.
//  2. Each trace step records the Quint state after an action (tryLinearize / backtrack).
//  3. We replay the trace's final `result` field against `check_operations`.
//  4. If the results differ, the Rust implementation diverges from the formal model.
//
// INV-LIN-01 (Soundness) and INV-LIN-02 (Completeness) are both exercised here.

#![cfg(feature = "quint-mbt")]

use porcupine::{CheckResult, Model, Operation};
use serde::Deserialize;
use std::process::Command;

// ---------------------------------------------------------------------------
// Minimal ITF trace types (JSON schema from quint run --out-itf)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ItfTrace {
    #[serde(rename = "states")]
    states: Vec<ItfState>,
}

#[derive(Debug, Deserialize)]
struct ItfState {
    #[serde(rename = "result")]
    result: String,
}

// ---------------------------------------------------------------------------
// Sequential register model (mirrors Porcupine.qnt)
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

// The example history from Porcupine.qnt (must match HISTORY in the .qnt file).
fn example_history() -> Vec<Operation<RegisterInput, i64>> {
    vec![
        Operation { client_id: 0, input: RegisterInput { is_write: true,  value: 1 }, call: 0,  output: 0, return_time: 10 },
        Operation { client_id: 1, input: RegisterInput { is_write: false, value: 0 }, call: 5,  output: 1, return_time: 15 },
        Operation { client_id: 2, input: RegisterInput { is_write: true,  value: 2 }, call: 12, output: 0, return_time: 20 },
        Operation { client_id: 3, input: RegisterInput { is_write: false, value: 0 }, call: 18, output: 2, return_time: 25 },
    ]
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn quint_result_to_check_result(s: &str) -> CheckResult {
    match s {
        "Ok"      => CheckResult::Ok,
        "Illegal" => CheckResult::Illegal,
        _         => CheckResult::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Run `quint run` to generate an ITF trace, then compare the final Quint `result`
/// against `check_operations` on the same history.
///
/// Exercises INV-LIN-01 (Soundness) and INV-LIN-02 (Completeness).
#[test]
fn mbt_trace_matches_rust_checker() {
    let trace_path = std::env::temp_dir().join("porcupine_mbt_trace.itf.json");
    let status = Command::new("quint")
        .args([
            "run",
            "tla/Porcupine.qnt",
            "--out-itf",
            trace_path.to_str().unwrap(),
            "--max-steps",
            "20",
        ])
        .status()
        .expect("Failed to run `quint` — is it installed? (npm install -g @informalsystems/quint)");

    assert!(status.success(), "`quint run` exited with non-zero status");

    let trace_json = std::fs::read_to_string(&trace_path)
        .expect("Failed to read ITF trace output");
    let trace: ItfTrace = serde_json::from_str(&trace_json)
        .expect("Failed to parse ITF trace JSON");

    let final_state = trace.states.last()
        .expect("ITF trace has no states");

    let quint_result = quint_result_to_check_result(&final_state.result);

    let history = example_history();
    let model = RegisterModel;
    let rust_result = porcupine::checker::check_operations(&model, &history);

    assert_eq!(
        rust_result, quint_result,
        "MBT mismatch: Quint says {:?}, Rust says {:?}. \
         The Rust implementation diverges from the formal model.",
        quint_result, rust_result
    );
}
