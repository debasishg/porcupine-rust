# porcupine-rust — Test Suite Reference

All tests, how to run them, what they verify, and which invariants they cover.

---

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Rust / Cargo | stable | `rustup update stable` |
| Quint CLI | ≥ 0.31.0 | `npm install -g @informalsystems/quint` |
| Java | ≥ 17 | Required by Apalache (invoked by `quint verify`) |
| `uv` (Python runner) | any recent | Auto-downloaded into `~/.cache/hegel` on first Hegel test run if not on `PATH`; or install per [astral.sh/uv](https://docs.astral.sh/uv/) |

Verify:

```bash
cargo --version
quint --version
java --version
```

---

## 1. Rust Unit Tests (`cargo test --lib`)

**Location**: `src/bitset.rs` (inline `#[cfg(test)]` module)

**Run**:

```bash
cargo test --lib
```

These tests cover the `Bitset` primitive used by the DFS cache.

| Test | What it checks |
|------|----------------|
| `test_set_clear` | `set` / `clear` / `popcnt` — bits at chunk boundaries (0, 63, 64, 127) |
| `test_hash_deterministic` | Two equal bitsets produce identical hashes; `equals` agrees |
| `test_clone_independence` | Cloned bitset does not share backing storage with the original |

**Expected output**: 3 tests, all passing.

---

## 1b. Rust Checker Unit Tests (`cargo test --lib -- checker`)

**Location**: `src/checker.rs` (inline `#[cfg(test)]` module)

**Run**:

```bash
cargo test --lib
```

These tests cover internal helpers (`make_entries`, `renumber`, `convert_entries`, `NodeArena`), the public `check_operations` and `check_events` entry points, and the timeout API.

### Internal helper tests (13)

| Test | What it checks |
|------|----------------|
| `make_entries_empty_produces_no_entries` | Empty input → empty output |
| `make_entries_single_op_produces_two_entries` | One op → one Call + one Return entry |
| `make_entries_call_before_return_at_equal_timestamps` | INV-HIST-02 tie-breaking: Call sorts before Return at equal timestamps |
| `make_entries_time_sorted_across_two_ops` | Two ops produce four entries in ascending time order |
| `make_entries_large_timestamps_do_not_overflow` | Timestamps near `u64::MAX` sort correctly without overflow |
| `renumber_empty_produces_empty` | Empty event list → empty output |
| `renumber_contiguous_ids_are_unchanged` | IDs already in 0..n pass through unchanged |
| `renumber_noncontiguous_ids_become_0_based` | Sparse IDs (e.g. 100, 999) are remapped to 0..n |
| `renumber_preserves_event_kind_and_payload` | Kind and input/output fields are not modified |
| `convert_entries_uses_slice_index_as_time` | Position in event slice becomes the logical timestamp |
| `convert_entries_maps_kinds_and_ids_correctly` | Call/Return kind and shared op id are mapped correctly |
| `arena_lift_and_unlift_restores_two_op_list` | lift+unlift is an identity on a two-op arena |
| `arena_nested_lift_unlift_restores_three_op_list` | Nested lift+unlift sequences restore full three-op arena |

### `check_operations` tests (15)

| Test | What it checks |
|------|----------------|
| `ops_empty_history_is_ok` | Empty history → Ok |
| `ops_single_write_is_ok` | Single write → Ok |
| `ops_single_read_returning_init_value_is_ok` | Single read returning init state → Ok |
| `ops_single_read_returning_wrong_value_is_illegal` | Single read with wrong value → Illegal |
| `ops_sequential_write_then_correct_read_is_ok` | Non-overlapping write→read with correct value → Ok |
| `ops_sequential_read_after_write_returning_stale_value_is_illegal` | Non-overlapping write→stale read → Illegal |
| `ops_concurrent_write_and_read_returning_written_value_is_ok` | Overlapping write+read (return written value) → Ok |
| `ops_concurrent_write_and_read_returning_init_value_is_ok` | Overlapping write+read (return init value) → Ok |
| `ops_read_starts_after_write_completes_returning_stale_is_illegal` | Strictly ordered write then stale read → Illegal |
| `ops_instantaneous_op_is_ok` | call == return_time → Ok |
| `ops_multiple_reads_all_return_init_before_any_write_is_ok` | Concurrent reads all returning 0 before any write → Ok |
| `ops_two_sequential_writes_then_wrong_read_is_illegal` | Two writes then stale read → Illegal |
| `ops_cache_pruning_does_not_cause_false_illegal` | Two identical writes hit cache; valid unexplored path not pruned → Ok |
| `ops_backtracking_finds_valid_ordering_after_failed_attempts` | DFS backtracks and finds valid ordering on second attempt → Ok |

### `check_events` tests (13)

Mirrors the `check_operations` suite for the event-based entry point. Plus:

| Test | What it checks |
|------|----------------|
| `events_noncontiguous_ids_produce_same_result_as_contiguous_ids` | IDs 100 and 999 produce same result as 0 and 1 |
| `events_agree_with_operations_on_linearizable_history` | Both APIs return Ok on the same history |
| `events_agree_with_operations_on_illegal_history` | Both APIs return Illegal on the same illegal history |
| `events_backtracking_finds_valid_ordering_after_failed_attempts` | DFS backtracks on event history |

### `check_events` with `partition_events` tests (3)

These live in `mod events_partition_tests` and are the first unit tests to exercise the `partition_events` path inside `check_events`, closing the coverage gap identified after commit 8.

| Test | What it checks |
|------|----------------|
| `check_events_partition_two_keys_ok` | Two-key KV history via `partition_events` → each partition is independently linearizable → `Ok` |
| `check_events_partition_detects_illegal_in_one_key` | Stale read on key 0 propagates `Illegal` even though key 1 is ok |
| `check_events_partition_concurrent_writes_ok` | Interleaved Call/Return events on two different keys each resolve correctly → `Ok` |

### Timeout tests (7)

| Test | What it checks |
|------|----------------|
| `timeout_zero_duration_returns_unknown_or_definitive` | `Duration::ZERO` never panics; result is Ok or Unknown |
| `timeout_very_long_does_not_affect_result` | A 60-second timeout on a fast history returns Ok |
| `timeout_very_long_does_not_affect_illegal_result` | A 60-second timeout on an illegal history returns Illegal |
| `timeout_none_matches_none_no_timeout` | `None` and a very long timeout agree on the same history |
| `timeout_events_very_long_does_not_affect_result` | Same guarantee for `check_events` with a long timeout |
| `timeout_unknown_tests::timeout_short_duration_returns_unknown` | `SlowModel.step` sleeps 50 ms; 2 ms timer fires first → `check_operations` returns `Unknown` definitively |
| `timeout_unknown_tests::timeout_short_duration_events_returns_unknown` | Same guarantee via `check_events` |

The two `timeout_unknown_tests` use `SlowModel` — a register whose `step()` unconditionally sleeps 50 ms — paired with a 2 ms timer, making `Unknown` deterministic: the timer always fires during the first `step()` call, and `to_check_result` checks `timed_out` before `ok`, so `Unknown` is returned regardless of whether the DFS reached completion.

### `partition_tests` — multi-partition dispatch tests (3)

These live in `mod partition_tests` and exercise partition splitting and rayon dispatch through the always-on parallel `check_operations` entry point.

| Test | What it checks |
|------|----------------|
| `two_partition_ok_history` | Two-key KV history → partition split + rayon dispatch → `Ok` |
| `two_partition_illegal_history` | One stale-read partition propagates `Illegal` for the whole check |
| `three_partitions_all_ok` | Three-key history exercises rayon dispatch across 3 independent partitions |

### `to_check_result_tests` — priority logic tests (4)

These live in `mod to_check_result_tests` and pin the `(ok, timed_out, definitive_illegal)` priority
ordering: `Illegal > Unknown > Ok`.

| Test | What it checks |
|------|----------------|
| `ok_when_dfs_completed_cleanly` | `(ok=true, timed_out=false, illegal=false)` → `Ok` |
| `unknown_when_only_timer_fired` | `(ok=false, timed_out=true, illegal=false)` → `Unknown` |
| `illegal_when_dfs_finished_no_timeout` | `(ok=false, timed_out=false, illegal=true)` → `Illegal` |
| `illegal_takes_priority_over_unknown` | `(ok=false, timed_out=true, illegal=true)` → `Illegal` |

**Expected output**:
- `cargo test --lib`: **60 tests**, all passing

---

## 2. Go Compatibility Tests (`cargo test --test go_compat`)

**Location**: `tests/go_compat.rs`

**Run**:

```bash
cargo test --test go_compat
```

These tests port the original Go porcupine test suite to Rust, covering all four models shipped with the Go library. All histories and expected results mirror the Go source exactly.

### 2.1 Register model (5 tests)

A single integer register (`State = i32`, `init = 0`). Operations: `Put(v)` (always ok, state → v) and `Get` (ok iff observed value equals state).

| Test | What it checks |
|------|----------------|
| `register_unrelated_ops_ok` | Concurrent reads and writes on separate values → `Ok` |
| `register_write_read_ok` | Sequential write then correct read → `Ok` |
| `register_concurrent_writes_ok` | Two concurrent writes, reads consistent with one of them → `Ok` |
| `register_illegal_history` | Sequential write then stale read → `Illegal` |
| `register_read_then_write_ok` | Read returns init, overlapping write, subsequent read returns write value → `Ok` |

### 2.2 Etcd / Jepsen traces (1 batch test)

| Test | What it checks |
|------|----------------|
| `etcd_all_files` | Loads all 102 Jepsen etcd log files from `test_data/jepsen/`; every file must return `Ok` |

### 2.3 KV model — with partitioning (6 tests)

`KvModel` maps `key → string` with three operations: `Get`, `Put`, `Append`. Partitioned by key; each sub-history is checked independently. Test data lives in `test_data/kv/`.

| Test | File | Expected |
|------|------|----------|
| `kv_c01_ok` | `c01-ok.txt` | `Ok` |
| `kv_c01_bad` | `c01-bad.txt` | `Illegal` |
| `kv_c10_ok` | `c10-ok.txt` | `Ok` |
| `kv_c10_bad` | `c10-bad.txt` | `Illegal` |
| `kv_c50_ok` | `c50-ok.txt` | `Ok` |
| `kv_c50_bad` | `c50-bad.txt` | `Illegal` |

### 2.4 KV model — without partitioning (2 tests + 2 ignored)

| Test | File | Expected | Status |
|------|------|----------|--------|
| `kv_no_partition_1_client_ok` | `c01-ok.txt` | `Ok` | active |
| `kv_no_partition_1_client_bad` | `c01-bad.txt` | `Illegal` | active |
| `kv_no_partition_10_clients_ok` | `c10-ok.txt` | `Ok` | `#[ignore]` — takes 60–90 s |
| `kv_no_partition_10_clients_bad` | `c10-bad.txt` | `Illegal` | `#[ignore]` — takes 60–90 s |

The ignored tests are expected: without key-partitioning, the 10-client history is too large for fast exploration. Partitioning is the intended path.

**Expected output**: **15 passed**, 2 ignored.

---

## 3. TiPocket Model Tests (`cargo test --test tipocket`)

**Location**: `tests/tipocket.rs`

**Run**:

```bash
cargo test --test tipocket
```

These tests port the three linearizability models used by [TiPocket](https://github.com/pingcap/tipocket), a chaos-engineering toolkit for TiDB, to verify its use of the porcupine API. TiPocket's models run against live TiDB; we port the model definitions and verify their semantics with hand-crafted `Operation` histories.

### 3.1 NoopModel — `pkg/check/porcupine/porcupine_test.go` (3 tests)

A single integer register initialised to `10`. Unknown responses are pass-throughs (state unchanged).

- **State**: `i32` (init = 10)
- **Input**: `NoopInput { op: u8, value: i32 }` — `0` = read, `1` = write
- **Output**: `NoopOutput { value: i32, unknown: bool }`

| Test | History | Expected |
|------|---------|----------|
| `noop_read_initial_ok` | Single read returning the initial value (10) | `Ok` |
| `noop_write_then_read_ok` | Sequential write(99), then read→99 | `Ok` |
| `noop_illegal_stale_read` | Sequential write(42), then read→10 (stale) | `Illegal` |

### 3.2 RawKvModel — `testcase/rawkv-linearizability/rawkv_linearizability.go` (6 tests)

Multi-key KV store partitioned by key. Missing keys implicitly return `0`. Three operations: Get (0), Put (1), Delete (2).

- **State**: `BTreeMap<i32, u32>` (init = empty)
- **Input**: `RawKvInput { op: u8, key: i32, val: u32 }`
- **Output**: `RawKvOutput { val: u32, unknown: bool }`
- **Partition**: groups operation indices by `input.key`

| Test | History | Expected |
|------|---------|----------|
| `rawkv_get_empty_ok` | Get on absent key returns 0 | `Ok` |
| `rawkv_put_then_get_ok` | Sequential put(key=1, val=42); get(key=1)→42 | `Ok` |
| `rawkv_delete_ok` | Sequential put, delete, get→0 | `Ok` |
| `rawkv_unknown_get_ok` | Concurrent put + get with unknown response | `Ok` |
| `rawkv_illegal_stale_get` | Sequential put(val=100); get→50 (wrong) | `Illegal` |
| `rawkv_two_key_partition_ok` | Four overlapping ops on two keys; partition splits correctly | `Ok` |

### 3.3 VBankModel — `testcase/vbank/client.go` (7 tests)

Virtual banking system with 10 accounts (IDs 0–9), each initial balance 20.0. Four operations: Read, Transfer, CreateAccount, DeleteAccount. Deleted account balances are consolidated into account 0. Failed (`ok=false`) and aborted operations leave state unchanged.

- **State**: `BTreeMap<i32, f64>` (init: id ∈ [0,9] → 20.0)
- **Input**: `VBankInput` enum with four variants
- **Output**: `VBankOutput { ok: bool, read_result: Option<BTreeMap<i32,f64>>, aborted: bool }`

| Test | History | Expected |
|------|---------|----------|
| `vbank_read_initial_ok` | Single read observing exact initial state | `Ok` |
| `vbank_transfer_ok` | Sequential transfer(0→1, 5.0); read showing updated balances | `Ok` |
| `vbank_create_account_ok` | Sequential create(id=10); read showing new account | `Ok` |
| `vbank_delete_account_ok` | Sequential delete(id=9); balance consolidated to account 0; read confirms | `Ok` |
| `vbank_illegal_stale_read` | Sequential transfer; read still showing pre-transfer balances | `Illegal` |
| `vbank_aborted_transfer_ok` | Concurrent aborted transfer + read of unchanged state | `Ok` |
| `vbank_failed_op_ok` | Sequential failed transfer (ok=false); read of unchanged state | `Ok` |

**Expected output**: **16 passed**, 0 ignored.

---

## 4. Property-Based Tests (`cargo test --test property_tests`)


**Location**: `tests/property_tests.rs`

**Run**:

```bash
cargo test --test property_tests
```

Uses [`proptest`](https://docs.rs/proptest) to generate random inputs. Each property is linked to one or more `INV-*` identifiers from `docs/spec.md`.

### 4.1 Sequential model

Most property tests use a simple integer-register model:

- **State**: `i64` (current register value, init `0`)
- **Write(v)**: always succeeds, transitions state to `v`
- **Read → v**: succeeds iff `v == current_state`

### 4.2 Nondeterministic models (INV-ND-01 tests)

Two `NondeterministicModel` implementations are used for the `prop_nd_*` tests:

- **`DeterministicNdRegister`**: wraps `RegisterModel` as a `NondeterministicModel`; step always returns exactly one successor (or empty). Used to verify that `PowerSetModel` of a degenerate ND model agrees with the equivalent deterministic `Model`.
- **`LossyNdRegister`**: a genuine branching model where a write of value `v` from state `s` may succeed (`→ v`) or be lost (`→ s`). Read must return exact current state.

### 4.3 Test inventory — sequential paths (default)

| Test | INV-* | Description |
|------|-------|-------------|
| `prop_well_formed_history` | INV-HIST-01 | Generated sequential histories satisfy `call ≤ return_time` for every operation |
| `prop_sequential_history_is_linearizable` | INV-LIN-01, INV-LIN-02 | A purely sequential (non-overlapping) history of 1–8 writes is always `CheckResult::Ok` |
| `prop_single_op_linearizable` | INV-LIN-01 | A single-operation history is trivially linearizable |
| `prop_compositionality_partitions_disjoint` | INV-LIN-03 | Partitions produced by `KvModel::partition` are disjoint and cover all operation indices |
| `prop_cache_sound_deterministic` | INV-LIN-04 | Two calls to `check_operations` with identical input always return the same result |
| `prop_illegal_history_is_detected` | INV-LIN-02 | A provably non-linearizable history (read after write returns stale value) returns `CheckResult::Illegal` |
| `prop_compositionality_end_to_end` | INV-LIN-03 | A sequential KV history checked through the partition path returns `CheckResult::Ok` |
| `prop_events_sequential_history_is_linearizable` | INV-LIN-01, INV-LIN-02 | Sequential history expressed as events is always `CheckResult::Ok` |
| `prop_events_single_op_is_linearizable` | INV-LIN-01 | Single-operation event history is trivially linearizable |
| `prop_events_agree_with_operations_on_sequential_history` | INV-LIN-01, INV-LIN-02 | `check_events` and `check_operations` must return the same result for the same sequential history |
| `prop_events_illegal_history_is_detected` | INV-LIN-02 | A non-linearizable event history returns `CheckResult::Illegal` |
| `prop_events_cache_sound_deterministic` | INV-LIN-04 | Two calls to `check_events` with identical input return the same result |
| `prop_events_empty_history_is_ok` | INV-LIN-01 | Empty event history returns `CheckResult::Ok` |

### 4.4 Test inventory — `NondeterministicModel` / `PowerSetModel` (INV-ND-01)

| Test | INV-* | Description |
|------|-------|-------------|
| `prop_nd_deterministic_agrees_with_model` | INV-ND-01 | `PowerSetModel(DeterministicNdRegister)` and `RegisterModel` return the same `CheckResult` for every sequential history |
| `prop_nd_sequential_writes_linearizable` | INV-ND-01, INV-LIN-01, INV-LIN-02 | A sequential write-only history through `LossyNdRegister` is always `Ok` |
| `prop_nd_single_op_is_linearizable` | INV-ND-01, INV-LIN-01 | A single-write operation is trivially linearizable under `LossyNdRegister` |
| `prop_nd_impossible_read_is_illegal` | INV-ND-01, INV-LIN-02 | A read of a value outside the reachable power-state is always `Illegal` |
| `prop_nd_cache_sound_deterministic` | INV-LIN-04 | Two calls to `check_operations` with the same ND history return the same result |

**Expected output**: 18 property tests, all passing.

### 4.5 The illegal history used in `prop_illegal_history_is_detected`

```
Client 0: write(1)  [0, 10]    — completes at t=10
Client 1: read → 0  [5, 15]    — overlaps write; may return 0 or 1 (ok)
Client 2: read → 0  [12, 20]   — starts AFTER write (t=12 > t=10); must return 1, not 0
```

This history has no valid linearization: `Illegal` is the only correct answer.

### 4.6 The KV model used in compositionality tests

`KvModel` maps `key → i64`. Its `partition` function groups operation indices by key, giving independent sub-histories — one per key. `check_operations` uses this partition internally when `Model::partition` returns `Some(_)`, checking all partitions concurrently via rayon.

---

## 4b. Hegel Property Tests (`cargo test --test hegel_properties`)

**Location**: `tests/hegel_properties.rs`

**Run**:

```bash
cargo test --test hegel_properties
```

Uses [Hegel](https://hegel.dev), Antithesis's universal property-based testing protocol — a [Hypothesis](https://github.com/hypothesisworks/hypothesis)-powered generator/shrinker engine with a Rust binding (the `hegeltest` crate, imported as `hegel`). On first run Hegel downloads a private copy of `uv` into `~/.cache/hegel` if `uv` is not already on `PATH`, then talks to a `hegel-core` Python sidecar over a local socket.

This suite mirrors the `proptest` suite (§4) using Hegel's generator/shrinker, plus a few properties that aren't covered by `proptest` (history prefixes, partition idempotence, an incremental stateful machine).

### 4b.1 Why Hegel in addition to proptest?

- Hypothesis's IR-level shrinker tends to find smaller counter-examples than proptest's value-level shrinker, especially for nested structures like operation histories.
- `#[hegel::state_machine]` provides first-class stateful PBT with the same shrinker; no extra crate needed.
- Tests are written against the same Hegel protocol available in Go, C++, TS, and OCaml — useful if the porcupine spec ever needs to be cross-validated against a non-Rust implementation.
- Hegel tests can be lifted into Antithesis's deterministic simulator without rewriting; each `tc.draw` becomes a controlled choice point.

Tradeoff: each test case round-trips through the `hegel-core` sidecar, so the suite is slower than proptest (~7.5 s for 17 × 100 cases). Keep proptest for fast inner-loop coverage; use Hegel as the deeper, slower gate.

For a deeper comparison — pros and cons of each engine, when to reach for which, and the rationale for keeping both — see [`docs/hegel_v_proptest.md`](./hegel_v_proptest.md).

### 4b.2 Models reused

The same `RegisterModel`, `KvModel`, `DeterministicNdRegister`, and `LossyNdRegister` types as `tests/property_tests.rs`, plus a `KvSinglePartition` wrapper used by `hegel_partition_idempotent_with_single_partition`.

### 4b.3 Test inventory — operation API

| Test | INV-* | Description |
|------|-------|-------------|
| `hegel_well_formed_history` | INV-HIST-01 | Generated sequential histories satisfy `call ≤ return_time` |
| `hegel_sequential_history_is_linearizable` | INV-LIN-01, INV-LIN-02 | Random sequential register histories of length 0–8 are always `Ok` |
| `hegel_single_op_is_linearizable` | INV-LIN-01 | Single-write history is trivially linearizable |
| `hegel_empty_history_is_ok` | INV-LIN-01 | Empty operation and event histories both return `Ok` |
| `hegel_prefixes_of_sequential_are_linearizable` | INV-LIN-01 | Every prefix of a generated sequential history is itself `Ok` |
| `hegel_illegal_history_is_detected` | INV-LIN-02 | Fixed three-op stale-read history returns `Illegal` |
| `hegel_stale_read_is_always_illegal` | INV-LIN-02 | Generative: a write of `v ≠ 0` followed by a read of `0` after the write completes is always `Illegal` |
| `hegel_partitions_are_disjoint_and_complete` | INV-LIN-03 | `KvModel::partition` produces disjoint, in-bounds, complete partitions |
| `hegel_kv_sequential_history_is_linearizable` | INV-LIN-03 | Sequential KV histories are `Ok` through the partition path |
| `hegel_partition_idempotent_with_single_partition` | INV-LIN-03 | A model whose `partition` returns one all-indices partition agrees with the same model with no partition |
| `hegel_cache_sound_deterministic_ops` | INV-LIN-04 | Two `check_operations` calls on the same history return the same result |

### 4b.4 Test inventory — event API

| Test | INV-* | Description |
|------|-------|-------------|
| `hegel_cache_sound_deterministic_events` | INV-LIN-04 | Two `check_events` calls on the same event stream return the same result |
| `hegel_events_agree_with_operations` | INV-LIN-01, INV-LIN-02 | `check_operations` and `check_events` agree on the same generated sequential history |

### 4b.5 Test inventory — `NondeterministicModel` / `PowerSetModel`

| Test | INV-* | Description |
|------|-------|-------------|
| `hegel_nd_deterministic_agrees_with_model` | INV-ND-01 | `PowerSetModel(DeterministicNdRegister)` and `RegisterModel` produce the same result on every generated history |
| `hegel_nd_sequential_writes_linearizable` | INV-ND-01, INV-LIN-01 | Sequential ND write histories through `LossyNdRegister` are always `Ok` |
| `hegel_nd_impossible_read_is_illegal` | INV-ND-01, INV-LIN-02 | A read of a value reachable in no branch of the lossy register is always `Illegal` |

### 4b.6 Stateful test (`#[hegel::state_machine]`)

| Test | INV-* | Description |
|------|-------|-------------|
| `hegel_incremental_register_is_linearizable` | INV-LIN-01 | A Hegel state machine grows a sequential register history one op at a time (`append_write`, `append_read_of_last`); after each rule the checker must report `Ok`. Surfaces any soundness bug whose effect depends on history length or interleaving order |

**Expected output**: 17 tests, all passing (≈ 7–10 s including the first-run `uv` install of ~5 MB).

---

## 5. Nondeterministic Model Tests (`cargo test --test nondeterministic`)

**Location**: `tests/nondeterministic.rs`

**Run**:

```bash
cargo test --test nondeterministic
```

Integration tests for the `NondeterministicModel` trait and `PowerSetModel` adapter
(both defined in `src/model.rs`).  Two concrete models are used:

- **`BranchingCounter`** — a counter that increments by either 1 or 2 on each step.
  Input: `()`. Output: `u32` (observed value).
- **`NdRegister`** — a lossy register where a write may succeed or be dropped.
  Input: `RegOp::Write(v) | RegOp::Read`. Output: `Option<u32>`.

### 5.1 Test inventory

| Test | Model | API | Expected |
|------|-------|-----|----------|
| `branching_counter_single_op_ok` | BranchingCounter | `check_operations` | `Ok` (output = 0+1) |
| `branching_counter_single_op_skip_ok` | BranchingCounter | `check_operations` | `Ok` (output = 0+2) |
| `branching_counter_single_op_illegal` | BranchingCounter | `check_operations` | `Illegal` (output = 3, impossible) |
| `branching_counter_sequential_ok` | BranchingCounter | `check_operations` | `Ok` (1 then 2) |
| `branching_counter_sequential_illegal` | BranchingCounter | `check_operations` | `Illegal` (1 then 4) |
| `branching_counter_concurrent_ok` | BranchingCounter | `check_operations` | `Ok` (overlapping A→B) |
| `nd_register_lossy_write_read_old_ok` | NdRegister | `check_operations` | `Ok` (write 42, read 0 — lost write) |
| `nd_register_lossy_write_read_new_ok` | NdRegister | `check_operations` | `Ok` (write 42, read 42 — write succeeded) |
| `nd_register_lossy_write_read_illegal` | NdRegister | `check_operations` | `Illegal` (write 42, read 99 — impossible) |
| `branching_counter_events_single_ok` | BranchingCounter | `check_events` | `Ok` |
| `branching_counter_events_single_illegal` | BranchingCounter | `check_events` | `Illegal` |
| `branching_counter_events_sequential_ok` | BranchingCounter | `check_events` | `Ok` (skip then increment) |

**Expected output**: 12 tests, all passing.

---

## 6. S2 Stream Model Tests (`cargo test --test s2_model`)

**Location**: `tests/s2_model.rs`

**Run**:

```bash
cargo test --test s2_model
```

Linearizability tests for [S2](https://s2.dev), an append-only stream-storage service.  The model
is a direct port of the Go `NondeterministicModel` in
[s2-streamstore/s2-verification](https://github.com/s2-streamstore/s2-verification).

### Why nondeterministic?

S2 appends can return `AppendIndefiniteFailure` (network or transient server error) — the write
may or may not have become durable.  The model returns both possible successor states for such
operations, which is why it implements `NondeterministicModel` and is wrapped in `PowerSetModel`
before being passed to `check_events`.

### State and I/O types

| Type | Description |
|------|-------------|
| `S2StreamState { tail, xxh3, fencing_token }` | `tail: u32` — next append offset; `xxh3: u64` — hash of last-appended batch; `fencing_token: Option<String>` — stream-level mutual-exclusion token |
| `S2Input` | `Append { num_records, xxh3, set_fencing_token, fencing_token, match_seq_num }` / `Read` / `CheckTail` |
| `S2Output` | `AppendSuccess { tail }` / `AppendDefiniteFailure` / `AppendIndefiniteFailure` / `ReadSuccess { tail, xxh3 }` / `ReadFailure` / `CheckTailSuccess { tail }` / `CheckTailFailure` |

### Step semantics

| Output | Semantics |
|--------|-----------|
| `AppendDefiniteFailure` | Guaranteed not durable → state unchanged |
| `AppendIndefiniteFailure` | May or may not be durable. If fencing token mismatches or `matchSeqNum` ≠ current tail, the write cannot have succeeded → state unchanged. Otherwise → both optimistic and original states are returned |
| `AppendSuccess { tail }` | Validates fencing token and `matchSeqNum` guards; verifies reported tail = `state.tail + num_records`; transitions to the optimistic state |
| `ReadSuccess { tail, xxh3 }` | `xxh3` must match current state; `tail` must equal current tail → state unchanged |
| `ReadFailure` / `CheckTailFailure` | State unchanged |
| `CheckTailSuccess { tail }` | `tail` must equal current tail → state unchanged |

### Test inventory

The first five tests are direct ports of the Go test suite in
`golang/s2-porcupine/main_test.go`.  The sixth test is an optional file-based integration test.

| Test | Go equivalent | Expected | Description |
|------|--------------|----------|-------------|
| `basic_no_concurrency` | `TestBasicNoConcurrency` | `Ok` | Sequential Append(4) → Read → CheckTail, all succeed |
| `definite_failure_linearizable` | `TestBasicNoConcurrencyDefiniteFailure1` | `Ok` | Definite-failed append; subsequent read returns pre-failure tail |
| `definite_failure_illegal` | `TestBasicNoConcurrencyDefiniteFailure2` | `Illegal` | Definite-failed append; subsequent read claims updated tail — impossible |
| `indefinite_failure_updated_tail_ok` | `TestBasicNoConcurrencyIndefiniteFailure1` | `Ok` | Indefinite failure; next read returns updated tail (failure became durable) |
| `indefinite_failure_original_tail_ok` | `TestBasicNoConcurrencyIndefiniteFailure2` | `Ok` | Indefinite failure; next read returns original tail (failure did not become durable) |
| `check_jsonl_file_if_present` | — | `Ok` | Reads `S2_HISTORY_FILE` env var (or `test_data/s2_records.jsonl`); skips silently if absent |

**Expected output**: **6 tests** passing (file-based test skips when no history file is present).

### Checking a real S2 history

**Step 1 — Collect a history** using `collect-history` from s2-verification:

```bash
# In the s2-streamstore/s2-verification repo:
cargo run --bin collect-history -- <basin> <stream> \
  --num-concurrent-clients 5 --num-ops-per-client 100 --workflow regular
# Produces: ./data/records.<timestamp>.jsonl
```

**Step 2 — Run the checker** (two options):

```bash
# Option A — via the file-based test (requires S2_HISTORY_FILE env var):
S2_HISTORY_FILE=path/to/records.jsonl cargo test --test s2_model check_jsonl_file_if_present

# Option B — via the CLI example binary:
cargo run --example s2_checker -- path/to/records.jsonl
# Optional timeout (seconds):
cargo run --example s2_checker -- path/to/records.jsonl 60
```

The `s2_checker` example exits 0 if the history is linearizable, 1 otherwise.

### JSONL format

The parser (`parse_s2_jsonl` in `tests/s2_model.rs` and `examples/s2_checker.rs`) handles the
mixed string/object event format emitted by `collect-history`:

```jsonl
{"event":{"Start":{"Append":{"num_records":4,"last_record_xxh3":12345,...}}},"client_id":0,"op_id":0}
{"event":{"Finish":{"AppendSuccess":{"tail":4}}},"client_id":0,"op_id":0}
{"event":{"Start":"Read"},"client_id":0,"op_id":1}
{"event":{"Finish":{"ReadSuccess":{"tail":4,"xxh3":12345}}},"client_id":0,"op_id":1}
{"event":{"Finish":"AppendDefiniteFailure"},"client_id":1,"op_id":2}
```

---

## 7. Model-Based Tests (`cargo test --features quint-mbt --test quint_mbt`)

**Location**: `tests/quint_mbt.rs`

**Feature flag**: `quint-mbt` (disabled by default).

**Run**:

```bash
cargo test --features quint-mbt --test quint_mbt
```

**Requires**: `quint` CLI on `PATH`.

### How it works

1. The test invokes `quint run tla/Porcupine.qnt --out-itf <tmp>.itf.json --max-steps 20`.
2. Quint executes the formal model and writes an ITF (Interaction Trace Format) JSON file recording each state.
3. The test reads the final state's `result` field (`"Ok"`, `"Illegal"`, or `"Unknown"`).
4. It runs the Rust checker on the same concrete history defined in `tla/Porcupine.qnt`:

```
op0: write(1)  [0, 10]
op1: read → 1  [5, 15]
op2: write(2)  [12, 20]
op3: read → 2  [18, 25]
```

5. The Quint result and the Rust result must agree. Any mismatch means the Rust implementation diverges from the formal model.

### Test inventory

| Test | Feature gate | INV-* | Description |
|------|-------------|-------|-------------|
| `mbt_trace_matches_rust_checker` | `quint-mbt` | INV-LIN-01, INV-LIN-02 | Runs `quint run`, reads final ITF state, asserts `check_operations` result matches Quint |
| `mbt_check_events_agrees_with_check_operations` | `quint-mbt` | INV-LIN-01, INV-LIN-02 | Pure Rust cross-API check: `check_events` and `check_operations` agree on the Quint example history |
| `mbt_check_events_matches_quint_trace` | `quint-mbt` | INV-LIN-01, INV-LIN-02 | Runs `quint run`, asserts `check_events` result matches Quint |

**Expected output**: `--features quint-mbt`: **3 tests**, all passing.

(The history above is linearizable → all checks return `Ok`.)

---

## 8. Quint Model Checking

### 8.1 DFS algorithm — `quint verify tla/Porcupine.qnt`

```bash
quint verify tla/Porcupine.qnt --invariant safetyInvariant
```

Checks the six sub-invariants covering INV-HIST-01, INV-HIST-03, INV-LIN-01–04, INV-PAR-01.

**Expected output**: `[ok] No violation found`

### 8.2 Power-set construction — `quint verify tla/NondeterministicModel.qnt`

```bash
quint verify tla/NondeterministicModel.qnt --invariant safetyInvariant
```

Checks INV-ND-01 (`powerSetSoundnessInv`): empty power-state implies rejection, and
acceptance implies non-empty power-state.  Models the `NdRegister` (lossy write).

**Expected output**: `[ok] No violation found`

---

## 9. Full Test Suites at a Glance

### Run everything (no Quint required)

```bash
cargo test
```

Runs all suites without feature flags: **153 tests** passing across eight test targets.

| Target | Command | Count |
|--------|---------|-------|
| Lib unit tests | `cargo test --lib` | 67 |
| Go compatibility | `cargo test --test go_compat` | 15 (+ 2 ignored) |
| TiPocket models | `cargo test --test tipocket` | 16 |
| Property tests (proptest) | `cargo test --test property_tests` | 18 |
| Property tests (Hegel) | `cargo test --test hegel_properties` | 17 |
| Nondeterministic model | `cargo test --test nondeterministic` | 12 |
| S2 stream model | `cargo test --test s2_model` | 6 |
| Doc tests | `cargo test --doc` | 2 |

rayon is an unconditional dependency; no feature flags are required for any of the above. The Hegel suite auto-installs `uv` into `~/.cache/hegel` on first run if `uv` is not already on `PATH`.

### Run everything including MBT (Quint required)

```bash
cargo test && cargo test --features quint-mbt --test quint_mbt
```

### Run Quint verification

```bash
quint verify tla/Porcupine.qnt --invariant safetyInvariant
```

### Full pre-merge suite (via skill)

```
/verify
```

---

## 10. Invariant Coverage Matrix

| INV-* | Name | Unit tests | proptest | Hegel | Quint invariant | MBT |
|-------|------|------------|----------|-------|-----------------|-----|
| INV-HIST-01 | Well-Formed History | — | `prop_well_formed_history` | `hegel_well_formed_history` | `Porcupine.qnt histWellFormedInv` | — |
| INV-HIST-02 | Real-Time Order | structural | `prop_sequential_history_is_linearizable` | `hegel_sequential_history_is_linearizable`, `hegel_stale_read_is_always_illegal` | `Porcupine.qnt realTimeOrder` (pure def) | — |
| INV-HIST-03 | Minimal-Call Frontier | structural | covered by soundness tests | covered by soundness tests | `Porcupine.qnt minimalCallFrontier` | — |
| INV-LIN-01 | Soundness | `timeout_very_long_does_not_affect_result`, `two_partition_ok_history` | `prop_sequential_history_is_linearizable`, `prop_single_op_linearizable` | `hegel_sequential_history_is_linearizable`, `hegel_single_op_is_linearizable`, `hegel_empty_history_is_ok`, `hegel_prefixes_of_sequential_are_linearizable`, `hegel_incremental_register_is_linearizable` | `Porcupine.qnt resultConsistent` | `mbt_trace_matches_rust_checker`, `mbt_check_events_matches_quint_trace` |
| INV-LIN-02 | Completeness | `timeout_very_long_does_not_affect_illegal_result`, `two_partition_illegal_history` | `prop_illegal_history_is_detected` | `hegel_illegal_history_is_detected`, `hegel_stale_read_is_always_illegal` | `Porcupine.qnt resultConsistent` | `mbt_trace_matches_rust_checker` |
| INV-LIN-03 | P-Compositionality | `check_events_partition_*`, `two_partition_*`, `three_partitions_all_ok` | `prop_compositionality_partitions_disjoint`, `prop_compositionality_end_to_end` | `hegel_partitions_are_disjoint_and_complete`, `hegel_kv_sequential_history_is_linearizable`, `hegel_partition_idempotent_with_single_partition` | `Porcupine.qnt pCompositionality` | — |
| INV-LIN-04 | Cache Soundness | `timeout_none_matches_none_no_timeout` | `prop_cache_sound_deterministic`, `prop_events_cache_sound_deterministic`, `prop_nd_cache_sound_deterministic` | `hegel_cache_sound_deterministic_ops`, `hegel_cache_sound_deterministic_events` | `Porcupine.qnt cacheSound` | — |
| INV-PAR-01 | Kill-Flag Monotonicity | `timeout_zero_duration_returns_unknown_or_definitive`, `timeout_short_duration_returns_unknown`, `timeout_short_duration_events_returns_unknown` | — | — | `Porcupine.qnt parallelKillFlagInvariant` | — |
| INV-ND-01 | Power-Set Reduction Soundness | structural (`PowerSetModel::step`) | `prop_nd_deterministic_agrees_with_model`, `prop_nd_sequential_writes_linearizable`, `prop_nd_single_op_is_linearizable`, `prop_nd_impossible_read_is_illegal` | `hegel_nd_deterministic_agrees_with_model`, `hegel_nd_sequential_writes_linearizable`, `hegel_nd_impossible_read_is_illegal` | `NondeterministicModel.qnt powerSetSoundnessInv` | — |

Full invariant definitions: `docs/spec.md`.
