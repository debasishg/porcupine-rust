//! Integration tests porting three TiPocket linearizability models.
//!
//! Sources (https://github.com/pingcap/tipocket):
//!   - NoopModel:  pkg/check/porcupine/porcupine_test.go
//!   - RawKvModel: testcase/rawkv-linearizability/rawkv_linearizability.go
//!   - VBankModel: testcase/vbank/client.go
//!
//! TiPocket tests run against live TiDB instances; we port the model definitions
//! and verify them with hand-crafted Operation histories, mirroring go_compat.rs.

#![allow(dead_code)]

use porcupine::checker::check_operations;
use porcupine::{CheckResult, Model, Operation};
use std::collections::{BTreeMap, HashMap};

// ============================================================================
// NOOP MODEL
// Mirrors TiPocket's `noop` model: a single integer register initialised to 10.
// Unknown responses are pass-throughs — the state is left unchanged.
//
// Source: pkg/check/porcupine/porcupine_test.go
// ============================================================================

#[derive(Clone)]
struct NoopModel;

#[derive(Clone, PartialEq)]
struct NoopInput {
    op: u8,    // 0 = read, 1 = write
    value: i32,
}

#[derive(Clone, PartialEq)]
struct NoopOutput {
    value: i32,
    unknown: bool,
}

impl Model for NoopModel {
    type State = i32;
    type Input = NoopInput;
    type Output = NoopOutput;

    fn init(&self) -> i32 {
        10
    }

    fn step(&self, state: &i32, input: &NoopInput, output: &NoopOutput) -> Option<i32> {
        if output.unknown {
            return Some(*state);
        }
        match input.op {
            0 => {
                // read: observed value must equal current state
                if output.value == *state {
                    Some(*state)
                } else {
                    None
                }
            }
            1 => {
                // write: next state is the written value (from response)
                Some(output.value)
            }
            _ => None,
        }
    }
}

fn noop_op(
    id: u64,
    op: u8,
    in_value: i32,
    out_value: i32,
    unknown: bool,
    call: u64,
    ret: u64,
) -> Operation<NoopInput, NoopOutput> {
    Operation {
        client_id: id,
        input: NoopInput { op, value: in_value },
        output: NoopOutput { value: out_value, unknown },
        call,
        return_time: ret,
    }
}

// --- Noop tests -------------------------------------------------------------

#[test]
fn noop_read_initial_ok() {
    // Single read returning the initial value (10). Always linearizable.
    let history = vec![noop_op(1, 0, 0, 10, false, 0, 5)];
    assert_eq!(check_operations(&NoopModel, &history, None), CheckResult::Ok);
}

#[test]
fn noop_write_then_read_ok() {
    // Sequential: write 99, then read 99. The write response carries the new value.
    let history = vec![
        noop_op(1, 1, 99, 99, false, 0, 5),   // write 99 → response value 99
        noop_op(2, 0, 0, 99, false, 10, 15),   // read → 99
    ];
    assert_eq!(check_operations(&NoopModel, &history, None), CheckResult::Ok);
}

#[test]
fn noop_illegal_stale_read() {
    // Sequential: write 42, then read 10 (stale — should be 42). Illegal.
    let history = vec![
        noop_op(1, 1, 42, 42, false, 0, 5),   // write 42
        noop_op(2, 0, 0, 10, false, 10, 15),   // read → 10 (stale)
    ];
    assert_eq!(
        check_operations(&NoopModel, &history, None),
        CheckResult::Illegal
    );
}

// ============================================================================
// RAW KV MODEL
// Mirrors TiPocket's `rawkvModel`: a multi-key KV store partitioned by key.
// Missing keys implicitly map to 0. Three operations: Get (0), Put (1), Delete (2).
// Unknown Get responses are always accepted (pass-through).
//
// Source: testcase/rawkv-linearizability/rawkv_linearizability.go
// ============================================================================

#[derive(Clone)]
struct RawKvModel;

#[derive(Clone, PartialEq)]
struct RawKvInput {
    op: u8,   // 0 = get, 1 = put, 2 = delete
    key: i32,
    val: u32,
}

#[derive(Clone, PartialEq)]
struct RawKvOutput {
    val: u32,
    unknown: bool,
}

impl Model for RawKvModel {
    type State = BTreeMap<i32, u32>;
    type Input = RawKvInput;
    type Output = RawKvOutput;

    fn init(&self) -> BTreeMap<i32, u32> {
        BTreeMap::new()
    }

    fn step(
        &self,
        state: &BTreeMap<i32, u32>,
        input: &RawKvInput,
        output: &RawKvOutput,
    ) -> Option<BTreeMap<i32, u32>> {
        match input.op {
            0 => {
                // get: unknown → pass-through; otherwise value must match
                if output.unknown {
                    return Some(state.clone());
                }
                let expected = state.get(&input.key).copied().unwrap_or(0);
                if output.val == expected {
                    Some(state.clone())
                } else {
                    None
                }
            }
            1 => {
                // put: always succeeds, insert/update key
                let mut next = state.clone();
                next.insert(input.key, input.val);
                Some(next)
            }
            2 => {
                // delete: always succeeds, remove key (missing key is a no-op)
                let mut next = state.clone();
                next.remove(&input.key);
                Some(next)
            }
            _ => None,
        }
    }

    fn partition(
        &self,
        history: &[Operation<RawKvInput, RawKvOutput>],
    ) -> Option<Vec<Vec<usize>>> {
        let mut by_key: HashMap<i32, Vec<usize>> = HashMap::new();
        for (i, op) in history.iter().enumerate() {
            by_key.entry(op.input.key).or_default().push(i);
        }
        Some(by_key.into_values().collect())
    }
}

fn rawkv_op(
    id: u64,
    op: u8,
    key: i32,
    in_val: u32,
    out_val: u32,
    unknown: bool,
    call: u64,
    ret: u64,
) -> Operation<RawKvInput, RawKvOutput> {
    Operation {
        client_id: id,
        input: RawKvInput { op, key, val: in_val },
        output: RawKvOutput { val: out_val, unknown },
        call,
        return_time: ret,
    }
}

// --- RawKv tests ------------------------------------------------------------

#[test]
fn rawkv_get_empty_ok() {
    // Get on an absent key returns 0. Trivially linearizable.
    let history = vec![rawkv_op(1, 0, 5, 0, 0, false, 0, 5)];
    assert_eq!(check_operations(&RawKvModel, &history, None), CheckResult::Ok);
}

#[test]
fn rawkv_put_then_get_ok() {
    // Sequential: put(key=1, val=42), then get(key=1) → 42.
    let history = vec![
        rawkv_op(1, 1, 1, 42, 0, false, 0, 5),    // put key=1 val=42
        rawkv_op(2, 0, 1, 0, 42, false, 10, 15),   // get key=1 → 42
    ];
    assert_eq!(check_operations(&RawKvModel, &history, None), CheckResult::Ok);
}

#[test]
fn rawkv_delete_ok() {
    // Sequential: put, delete, get → 0 (key absent after delete).
    let history = vec![
        rawkv_op(1, 1, 3, 7, 0, false, 0, 5),    // put key=3 val=7
        rawkv_op(2, 2, 3, 0, 0, false, 10, 15),   // delete key=3
        rawkv_op(3, 0, 3, 0, 0, false, 20, 25),   // get key=3 → 0
    ];
    assert_eq!(check_operations(&RawKvModel, &history, None), CheckResult::Ok);
}

#[test]
fn rawkv_unknown_get_ok() {
    // Put and get are concurrent; the get has an unknown response.
    // Unknown responses are always valid — the checker accepts them regardless of state.
    let history = vec![
        rawkv_op(1, 1, 2, 99, 0, false, 0, 20),   // put key=2 val=99 (long window)
        rawkv_op(2, 0, 2, 0, 0, true, 5, 15),     // get key=2 → unknown
    ];
    assert_eq!(check_operations(&RawKvModel, &history, None), CheckResult::Ok);
}

#[test]
fn rawkv_illegal_stale_get() {
    // Sequential: put(key=7, val=100), then get returns 50. Illegal.
    let history = vec![
        rawkv_op(1, 1, 7, 100, 0, false, 0, 5),   // put key=7 val=100
        rawkv_op(2, 0, 7, 0, 50, false, 10, 15),  // get key=7 → 50 (wrong)
    ];
    assert_eq!(
        check_operations(&RawKvModel, &history, None),
        CheckResult::Illegal
    );
}

#[test]
fn rawkv_two_key_partition_ok() {
    // All four operations overlap in time. The partition splits by key:
    //   key=1 partition: put(1,10), get(1)→10
    //   key=2 partition: put(2,20), get(2)→20
    // Each sub-history is independently linearizable.
    let history = vec![
        rawkv_op(1, 1, 1, 10, 0, false, 0, 30),    // put key=1 val=10
        rawkv_op(2, 1, 2, 20, 0, false, 0, 30),    // put key=2 val=20
        rawkv_op(3, 0, 1, 0, 10, false, 5, 25),    // get key=1 → 10
        rawkv_op(4, 0, 2, 0, 20, false, 5, 25),    // get key=2 → 20
    ];
    assert_eq!(check_operations(&RawKvModel, &history, None), CheckResult::Ok);
}

// ============================================================================
// VBANK MODEL
// Mirrors TiPocket's virtual bank model: 10 accounts (IDs 0–9), each starting
// with balance 20.0. Supports Read, Transfer, CreateAccount, and DeleteAccount.
// Deleted account balances are consolidated into account 0.
// Failed (!ok) and aborted operations leave state unchanged.
//
// Source: testcase/vbank/client.go
// ============================================================================

#[derive(Clone)]
struct VBankModel;

#[derive(Clone, PartialEq)]
enum VBankInput {
    Read,
    Transfer { from_id: i32, to_id: i32, amount: f64 },
    CreateAccount { new_id: i32 },
    DeleteAccount { victim_id: i32 },
}

#[derive(Clone, PartialEq)]
struct VBankOutput {
    ok: bool,
    /// Populated for Read operations: the observed account state.
    read_result: Option<BTreeMap<i32, f64>>,
    /// True when a Transfer or DeleteAccount was rolled back.
    aborted: bool,
}

impl Model for VBankModel {
    type State = BTreeMap<i32, f64>;
    type Input = VBankInput;
    type Output = VBankOutput;

    fn init(&self) -> BTreeMap<i32, f64> {
        (0..10).map(|i| (i, 20.0)).collect()
    }

    fn step(
        &self,
        state: &BTreeMap<i32, f64>,
        input: &VBankInput,
        output: &VBankOutput,
    ) -> Option<BTreeMap<i32, f64>> {
        if !output.ok {
            // Failed operation: state unchanged, always valid.
            return Some(state.clone());
        }
        match input {
            VBankInput::Read => {
                // Observed state must exactly match model state.
                match &output.read_result {
                    Some(observed) if observed == state => Some(state.clone()),
                    _ => None,
                }
            }
            VBankInput::Transfer { from_id, to_id, amount } => {
                if output.aborted {
                    return Some(state.clone());
                }
                let mut next = state.clone();
                *next.entry(*from_id).or_insert(0.0) -= amount;
                *next.entry(*to_id).or_insert(0.0) += amount;
                Some(next)
            }
            VBankInput::CreateAccount { new_id } => {
                let mut next = state.clone();
                next.insert(*new_id, 10.0);
                Some(next)
            }
            VBankInput::DeleteAccount { victim_id } => {
                if output.aborted {
                    return Some(state.clone());
                }
                let mut next = state.clone();
                let victim_balance = next.remove(victim_id).unwrap_or(0.0);
                // Consolidate deleted account's balance into account 0.
                *next.entry(0).or_insert(0.0) += victim_balance;
                Some(next)
            }
        }
    }
}

fn vbank_op(
    id: u64,
    input: VBankInput,
    output: VBankOutput,
    call: u64,
    ret: u64,
) -> Operation<VBankInput, VBankOutput> {
    Operation { client_id: id, input, output, call, return_time: ret }
}

fn init_accounts() -> BTreeMap<i32, f64> {
    (0..10).map(|i| (i, 20.0)).collect()
}

fn read_ok(accounts: BTreeMap<i32, f64>) -> VBankOutput {
    VBankOutput { ok: true, read_result: Some(accounts), aborted: false }
}

fn transfer_ok() -> VBankOutput {
    VBankOutput { ok: true, read_result: None, aborted: false }
}

fn transfer_aborted() -> VBankOutput {
    VBankOutput { ok: true, read_result: None, aborted: true }
}

fn op_failed() -> VBankOutput {
    VBankOutput { ok: false, read_result: None, aborted: false }
}

fn create_ok() -> VBankOutput {
    VBankOutput { ok: true, read_result: None, aborted: false }
}

fn delete_ok() -> VBankOutput {
    VBankOutput { ok: true, read_result: None, aborted: false }
}

// --- VBank tests ------------------------------------------------------------

#[test]
fn vbank_read_initial_ok() {
    // Single read that observes the exact initial state.
    let history = vec![vbank_op(
        1,
        VBankInput::Read,
        read_ok(init_accounts()),
        0,
        5,
    )];
    assert_eq!(check_operations(&VBankModel, &history, None), CheckResult::Ok);
}

#[test]
fn vbank_transfer_ok() {
    // Sequential: transfer 5.0 from account 0 to account 1,
    // then read showing the updated balances.
    let mut after_transfer = init_accounts();
    *after_transfer.get_mut(&0).unwrap() -= 5.0;
    *after_transfer.get_mut(&1).unwrap() += 5.0;

    let history = vec![
        vbank_op(
            1,
            VBankInput::Transfer { from_id: 0, to_id: 1, amount: 5.0 },
            transfer_ok(),
            0,
            5,
        ),
        vbank_op(2, VBankInput::Read, read_ok(after_transfer), 10, 15),
    ];
    assert_eq!(check_operations(&VBankModel, &history, None), CheckResult::Ok);
}

#[test]
fn vbank_create_account_ok() {
    // Sequential: create account 10 (balance 10.0), then read showing it.
    let mut after_create = init_accounts();
    after_create.insert(10, 10.0);

    let history = vec![
        vbank_op(1, VBankInput::CreateAccount { new_id: 10 }, create_ok(), 0, 5),
        vbank_op(2, VBankInput::Read, read_ok(after_create), 10, 15),
    ];
    assert_eq!(check_operations(&VBankModel, &history, None), CheckResult::Ok);
}

#[test]
fn vbank_delete_account_ok() {
    // Sequential: delete account 9. Its balance (20.0) is added to account 0.
    // After deletion: account 0 has 40.0, accounts 1–8 have 20.0, account 9 is gone.
    let mut after_delete = init_accounts();
    after_delete.remove(&9);
    *after_delete.get_mut(&0).unwrap() += 20.0;

    let history = vec![
        vbank_op(1, VBankInput::DeleteAccount { victim_id: 9 }, delete_ok(), 0, 5),
        vbank_op(2, VBankInput::Read, read_ok(after_delete), 10, 15),
    ];
    assert_eq!(check_operations(&VBankModel, &history, None), CheckResult::Ok);
}

#[test]
fn vbank_illegal_stale_read() {
    // Sequential: transfer 5.0 (account 0→1), then read the old (pre-transfer) state.
    // Illegal: the read cannot be linearized before the transfer because it's sequential.
    let history = vec![
        vbank_op(
            1,
            VBankInput::Transfer { from_id: 0, to_id: 1, amount: 5.0 },
            transfer_ok(),
            0,
            5,
        ),
        vbank_op(
            2,
            VBankInput::Read,
            read_ok(init_accounts()), // wrong: still shows pre-transfer balances
            10,
            15,
        ),
    ];
    assert_eq!(
        check_operations(&VBankModel, &history, None),
        CheckResult::Illegal
    );
}

#[test]
fn vbank_aborted_transfer_ok() {
    // Concurrent: an aborted transfer and a read that sees the initial state.
    // Aborted transfer leaves state unchanged, so the read is valid.
    let history = vec![
        vbank_op(
            1,
            VBankInput::Transfer { from_id: 3, to_id: 4, amount: 10.0 },
            transfer_aborted(),
            0,
            20,
        ),
        vbank_op(2, VBankInput::Read, read_ok(init_accounts()), 5, 15),
    ];
    assert_eq!(check_operations(&VBankModel, &history, None), CheckResult::Ok);
}

#[test]
fn vbank_failed_op_ok() {
    // Sequential: a failed transfer (ok=false) leaves state unchanged.
    // Subsequent read seeing the initial state is valid.
    let history = vec![
        vbank_op(
            1,
            VBankInput::Transfer { from_id: 0, to_id: 1, amount: 5.0 },
            op_failed(),
            0,
            5,
        ),
        vbank_op(2, VBankInput::Read, read_ok(init_accounts()), 10, 15),
    ];
    assert_eq!(check_operations(&VBankModel, &history, None), CheckResult::Ok);
}
