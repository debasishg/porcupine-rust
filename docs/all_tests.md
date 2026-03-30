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

### 2.2 Test inventory

| Test | INV-* | Description |
|------|-------|-------------|
| `prop_well_formed_history` | INV-HIST-01 | Generated sequential histories satisfy `call ≤ return_time` for every operation |
| `prop_sequential_history_is_linearizable` | INV-LIN-01, INV-LIN-02 | A purely sequential (non-overlapping) history of 1–8 writes is always `CheckResult::Ok` |
| `prop_single_op_linearizable` | INV-LIN-01 | A single-operation history is trivially linearizable |
| `prop_compositionality_partitions_disjoint` | INV-LIN-03 | Partitions produced by `KvModel::partition` are disjoint and cover all operation indices |
| `prop_cache_sound_deterministic` | INV-LIN-04 | Two calls to `check_operations` with identical input always return the same result |
| `prop_illegal_history_is_detected` | INV-LIN-02 | A provably non-linearizable history (read after write returns stale value) returns `CheckResult::Illegal` |
| `prop_compositionality_end_to_end` | INV-LIN-03 | A sequential KV history checked through the partition path returns `CheckResult::Ok` |

### 2.3 The illegal history used in `prop_illegal_history_is_detected`

```
Client 0: write(1)  [0, 10]    — completes at t=10
Client 1: read → 0  [5, 15]    — overlaps write; may return 0 or 1 (ok)
Client 2: read → 0  [12, 20]   — starts AFTER write (t=12 > t=10); must return 1, not 0
```

This history has no valid linearization: `Illegal` is the only correct answer.

### 2.4 The KV model used in compositionality tests

`KvModel` maps `key → i64`. Its `partition` function groups operation indices by key, giving independent sub-histories — one per key. `check_operations` uses this partition internally when `Model::partition` returns `Some(_)`.

**Expected output**: 7 tests, all passing.

---

## 3. Model-Based Tests (`cargo test --features quint-mbt --test quint_mbt`)

**Location**: `tests/quint_mbt.rs`

**Feature flag**: `quint-mbt` (disabled by default)

**Run**:

```bash
cargo test --features quint-mbt --test quint_mbt
```

**Requires**: `quint` CLI on `PATH`.

### How it works

1. The test invokes `quint run tla/Porcupine.qnt --out-itf <tmp>.itf.json --max-steps 20`.
2. Quint executes the formal model and writes an ITF (Interaction Trace Format) JSON file recording each state.
3. The test reads the final state's `result` field (`"Ok"`, `"Illegal"`, or `"Unknown"`).
4. It runs `check_operations` on the same concrete history defined in `tla/Porcupine.qnt`:

```
op0: write(1)  [0, 10]
op1: read → 1  [5, 15]
op2: write(2)  [12, 20]
op3: read → 2  [18, 25]
```

5. The Quint result and the Rust result must agree. Any mismatch means the Rust implementation diverges from the formal model.

### Test inventory

| Test | INV-* | Description |
|------|-------|-------------|
| `mbt_trace_matches_rust_checker` | INV-LIN-01, INV-LIN-02 | Replays the final state of a Quint ITF trace; asserts Quint and Rust agree on `Ok`/`Illegal` |

**Expected output**: 1 test, passing (history above is linearizable → both sides return `Ok`).

---

## 4. Quint Model Checking (`quint verify`)

**Location**: `tla/Porcupine.qnt`

**Run**:

```bash
quint verify tla/Porcupine.qnt --invariant safetyInvariant
```

This invokes **Apalache** (bundled with Quint) to perform bounded model checking over all reachable states up to `--max-steps` (default: 10).

### What is verified

`safetyInvariant` is the conjunction of four sub-invariants, each corresponding to an `INV-*` entry in `docs/spec.md`:

| Sub-invariant | INV-* | Condition |
|---------------|-------|-----------|
| `histWellFormedInv` | INV-HIST-01 | All operations in `HISTORY` have `call_ts ≥ 0` and `ret_ts ≥ call_ts` |
| `minimalCallFrontier` | INV-HIST-03 | Every operation in the frontier (eligible for linearization) is truly minimal — no unlinearized earlier operation exists |
| `cacheSound` | INV-LIN-04 | No two frames on the DFS stack share the same `op_id` (unique linearized set per stack depth) |
| `resultConsistent` | INV-LIN-01, INV-LIN-02 | `result = "Ok"` implies all ops linearized; `result = "Illegal"` implies frontier is empty |

The model also guards the `step` action with `result != "Unknown"` to stutter once the DFS terminates, preventing post-termination state mutations from violating `resultConsistent`.

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

Runs all lib unit tests and all integration tests (excluding `quint-mbt`).

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

## 7. Invariant Coverage Matrix

| INV-* | Name | Unit tests | Property tests | Quint invariant | MBT |
|-------|------|------------|----------------|-----------------|-----|
| INV-HIST-01 | Well-Formed History | — | `prop_well_formed_history` | `histWellFormedInv` | — |
| INV-HIST-02 | Real-Time Order | structural | `prop_sequential_history_is_linearizable` | `realTimeOrder` (pure def) | — |
| INV-HIST-03 | Minimal-Call Frontier | structural | covered by soundness tests | `minimalCallFrontier` | — |
| INV-LIN-01 | Soundness | — | `prop_sequential_history_is_linearizable`, `prop_single_op_linearizable` | `resultConsistent` | `mbt_trace_matches_rust_checker` |
| INV-LIN-02 | Completeness | — | `prop_illegal_history_is_detected` | `resultConsistent` | `mbt_trace_matches_rust_checker` |
| INV-LIN-03 | P-Compositionality | — | `prop_compositionality_partitions_disjoint`, `prop_compositionality_end_to_end` | `pCompositionality` (pure def) | — |
| INV-LIN-04 | Cache Soundness | — | `prop_cache_sound_deterministic` | `cacheSound` | — |

Full invariant definitions: `docs/spec.md`.
