# porcupine-rust — Test Suite Reference

All tests, how to run them, what they verify, and which invariants they cover.

---

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Rust / Cargo | stable | `rustup update stable` |
| Quint CLI | ≥ 0.31.0 | `npm install -g @informalsystems/quint` |
| Java | ≥ 17 | Required by Apalache (invoked by `quint verify`) |

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

## 2. Property-Based Tests (`cargo test --test property_tests`)

**Location**: `tests/property_tests.rs`

**Run**:

```bash
cargo test --test property_tests
```

Uses [`proptest`](https://docs.rs/proptest) to generate random inputs. Each property is linked to one or more `INV-*` identifiers from `docs/spec.md`.

### 2.1 Sequential model

All property tests use a simple integer-register model:

- **State**: `i64` (current register value, init `0`)
- **Write(v)**: always succeeds, transitions state to `v`
- **Read → v**: succeeds iff `v == current_state`

### 2.2 Test inventory — sequential paths (default)

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

**Expected output (default)**: 13 tests, all passing.

### 2.3 Test inventory — parallel paths (`--features parallel`)

These tests live in `mod parallel_tests` inside `tests/property_tests.rs` and are compiled only when the `parallel` feature is enabled.

**Run**:

```bash
cargo test --features parallel --test property_tests
```

| Test | INV-* | Description |
|------|-------|-------------|
| `parallel_tests::prop_parallel_ops_agrees_with_sequential` | INV-LIN-01, INV-LIN-02 | `check_operations_parallel` returns the same result as `check_operations` for any register history |
| `parallel_tests::parallel_detects_illegal_ops_history` | INV-LIN-02 | `check_operations_parallel` detects the known non-linearizable register history |
| `parallel_tests::prop_parallel_events_agrees_with_sequential` | INV-LIN-01, INV-LIN-02 | `check_events_parallel` returns the same result as `check_events` for any register event history |
| `parallel_tests::prop_parallel_kv_agrees_with_sequential` | INV-LIN-03 | For a multi-key KV history with partitioning, `check_operations_parallel` and `check_operations` agree — exercises real partition-level parallelism |

**Expected output (with `--features parallel`)**: 17 tests total (13 existing + 4 parallel), all passing.

### 2.4 The illegal history used in `prop_illegal_history_is_detected`

```
Client 0: write(1)  [0, 10]    — completes at t=10
Client 1: read → 0  [5, 15]    — overlaps write; may return 0 or 1 (ok)
Client 2: read → 0  [12, 20]   — starts AFTER write (t=12 > t=10); must return 1, not 0
```

This history has no valid linearization: `Illegal` is the only correct answer.

### 2.5 The KV model used in compositionality tests

`KvModel` maps `key → i64`. Its `partition` function groups operation indices by key, giving independent sub-histories — one per key. `check_operations` (and `check_operations_parallel`) use this partition internally when `Model::partition` returns `Some(_)`. The `prop_parallel_kv_agrees_with_sequential` test exercises actual rayon parallelism through this model.

---

## 3. Model-Based Tests (`cargo test --features quint-mbt --test quint_mbt`)

**Location**: `tests/quint_mbt.rs`

**Feature flag**: `quint-mbt` (disabled by default). Two tests are additionally gated on `feature = "parallel"`.

**Run**:

```bash
# Sequential paths only
cargo test --features quint-mbt --test quint_mbt

# Sequential + parallel paths
cargo test --features quint-mbt,parallel --test quint_mbt
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
| `mbt_parallel_ops_matches_quint_trace` | `quint-mbt` + `parallel` | INV-LIN-01, INV-LIN-02 | Runs `quint run`, asserts `check_operations_parallel` result matches Quint — pins the rayon path against the formal model |
| `mbt_parallel_events_matches_quint_trace` | `quint-mbt` + `parallel` | INV-LIN-01, INV-LIN-02 | Runs `quint run`, asserts `check_events_parallel` result matches Quint — pins both the event pipeline and rayon path simultaneously |

**Expected output**:
- `--features quint-mbt`: 3 tests, all passing
- `--features quint-mbt,parallel`: 5 tests, all passing

(The history above is linearizable → all checks return `Ok`.)

---

## 4. Quint Model Checking (`quint verify`)

**Location**: `tla/Porcupine.qnt`

**Run**:

```bash
quint verify tla/Porcupine.qnt --invariant safetyInvariant
```

This invokes **Apalache** (bundled with Quint) to perform bounded model checking over all reachable states up to `--max-steps` (default: 10).

### What is verified

`safetyInvariant` is the conjunction of six sub-invariants, each corresponding to an `INV-*` entry in `docs/spec.md`:

| Sub-invariant | INV-* | Condition |
|---------------|-------|-----------|
| `histWellFormedInv` | INV-HIST-01 | All operations in `HISTORY` have `call_ts ≥ 0` and `ret_ts ≥ call_ts` |
| `minimalCallFrontier` | INV-HIST-03 | Every operation in the frontier (eligible for linearization) is truly minimal — no unlinearized earlier operation exists |
| `cacheSound` | INV-LIN-04 | No two frames on the DFS stack share the same `op_id` (unique linearized set per stack depth) |
| `resultConsistent` | INV-LIN-01, INV-LIN-02 | `result = "Ok"` implies all ops linearized; `result = "Illegal"` implies frontier is empty |
| `pCompositionality` | INV-LIN-03 | When `result = "Ok"`, applying the ops in stack order from `INIT_VAL` is accepted by the register model at every step — the stack records a valid sequential execution |
| `parallelKillFlagInvariant` | INV-PAR-01 | `result` is always one of `"Unknown"`, `"Ok"`, or `"Illegal"` (models the write-once kill-flag monotonicity of the parallel implementation) |

The model also guards the `step` action with `result != "Unknown"` to stutter once the DFS terminates, preventing post-termination state mutations from violating `resultConsistent` and `pCompositionality`.

**Expected output**:

```
[ok] No violation found
```

### Concrete history modelled

```
op0: write(1)  [0, 10]
op1: read → 1  [5, 15]
op2: write(2)  [12, 20]
op3: read → 2  [18, 25]
```

This history is linearizable (`op0 → op1 → op2 → op3`). The model checker confirms no invariant is violated along any execution path.

---

## 5. Quint Simulation (`quint run`)

**Run**:

```bash
quint run tla/Porcupine.qnt
```

Runs a single randomised simulation (not exhaustive). Useful for:

- Generating ITF traces for MBT (used by `tests/quint_mbt.rs`)
- Sanity-checking model behaviour interactively

To emit a trace file explicitly:

```bash
quint run tla/Porcupine.qnt --out-itf /tmp/porcupine_trace.itf.json --max-steps 20
```

---

## 6. Full Test Suites at a Glance

### Run everything (no Quint required)

```bash
cargo test
```

Runs all lib unit tests and all integration tests (excluding `quint-mbt` and `parallel`).

### Run with parallel feature enabled

```bash
cargo test --features parallel
```

Adds 4 parallel property tests (17 total instead of 13). No Quint required.

### Run everything including MBT (Quint required)

```bash
cargo test && cargo test --features quint-mbt --test quint_mbt
```

### Run everything including MBT + parallel (Quint required)

```bash
cargo test --features parallel && cargo test --features quint-mbt,parallel --test quint_mbt
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

## 7. Invariant Coverage Matrix

Tests marked `[par]` require `--features parallel`.

| INV-* | Name | Unit tests | Property tests | Quint invariant | MBT |
|-------|------|------------|----------------|-----------------|-----|
| INV-HIST-01 | Well-Formed History | — | `prop_well_formed_history` | `histWellFormedInv` | — |
| INV-HIST-02 | Real-Time Order | structural | `prop_sequential_history_is_linearizable` | `realTimeOrder` (pure def) | — |
| INV-HIST-03 | Minimal-Call Frontier | structural | covered by soundness tests | `minimalCallFrontier` | — |
| INV-LIN-01 | Soundness | — | `prop_sequential_history_is_linearizable`, `prop_single_op_linearizable`, `prop_parallel_ops_agrees_with_sequential` [par], `prop_parallel_events_agrees_with_sequential` [par] | `resultConsistent` | `mbt_trace_matches_rust_checker`, `mbt_parallel_ops_matches_quint_trace` [par], `mbt_parallel_events_matches_quint_trace` [par] |
| INV-LIN-02 | Completeness | — | `prop_illegal_history_is_detected`, `parallel_detects_illegal_ops_history` [par] | `resultConsistent` | `mbt_trace_matches_rust_checker`, `mbt_parallel_ops_matches_quint_trace` [par] |
| INV-LIN-03 | P-Compositionality | — | `prop_compositionality_partitions_disjoint`, `prop_compositionality_end_to_end`, `prop_parallel_kv_agrees_with_sequential` [par] | `pCompositionality` | — |
| INV-LIN-04 | Cache Soundness | — | `prop_cache_sound_deterministic`, `prop_events_cache_sound_deterministic` | `cacheSound` | — |
| INV-PAR-01 | Kill-Flag Monotonicity | structural (`AtomicBool`, stutter guard) | — | `parallelKillFlagInvariant` | — |

Full invariant definitions: `docs/spec.md`.
