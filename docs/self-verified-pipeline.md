# Invariant Validation Pipeline: From Spec to Verification

> **Last updated**: 2026-03-30 | **Quint**: ≥ 0.31.0 | **quint-connect**: 0.1.x

## A Visual Journey of a Domain Invariant Through the Pipeline

---

## 1. The Complete Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        HUMAN-AUTHORED (Source of Truth)                     │
│                                                                             │
│   ┌─────────────────────────────────────────────────────────┐               │
│   │  docs/spec.md                                           │               │
│   │                                                         │               │
│   │  ### INV-LIN-01: Soundness                              │               │
│   │  ```                                                    │               │
│   │  check_operations = Ok  →  history is linearizable      │               │
│   │  ```                                                    │               │
│   │  If the checker returns Ok, a valid linearization exists│               │
│   └────────────────────────┬────────────────────────────────┘               │
└────────────────────────────┼────────────────────────────────────────────────┘
                             │
                             │  LLM reads spec, generates all downstream artifacts
                             │
        ┌────────────────────┼──────────────────────────────────────┐
        │                    ▼                                      │
        │  ┌──────────────────────────────┐                         │
        │  │  1. tla/Porcupine.qnt        │  Formal model           │
        │  │     (Quint spec)             │  (machine-checkable)    │
        │  └──────────┬───────────────────┘                         │
        │             │                                             │
        │    ┌────────┴────────┐                                    │
        │    ▼                 ▼                                    │
        │  ┌───────────┐  ┌──────────────┐                          │
        │  │2. invar-  │  │3. property_  │                          │
        │  │  iants.rs │  │  tests.rs    │                          │
        │  │  (macros) │  │  (proptest)  │                          │
        │  └─────┬─────┘  └──────┬───────┘                          │
        │        │               │                                  │
        │        ▼               │         ┌──────────────┐         │
        │  ┌───────────┐         │         │4. quint_     │         │
        │  │ checker.rs│         │         │  mbt.rs      │         │
        │  │ (DFS)     │         │         │  (MBT driver)│         │
        │  └───────────┘         │         └──────┬───────┘         │
        │                        │                │                 │
        │            LLM-GENERATED ARTIFACTS      │                 │
        └────────────────────────┼────────────────┼─────────────────┘
                                 │                │
                                 ▼                ▼
                     ┌───────────────────────────────────┐
                     │     VERIFICATION FEEDBACK LOOP    │
                     │                                   │
                     │  quint verify ← spec correctness  │
                     │  cargo test   ← impl correctness  │
                     │  debug_assert ← runtime checking  │
                     └───────────────────────────────────┘
```

---

## 2. Stage 1: The Human Specification (Source of Truth)

**File**: `docs/spec.md`

The invariant begins as a precise English statement with mathematical notation:

```
┌─────────────────────────────────────────────────────────────────┐
│  spec.md  §2 — Linearizability Invariants                       │
│                                                                 │
│  ### INV-LIN-01: Soundness                                      │
│  ┌───────────────────────────────────────────────────────┐      │
│  │  check_operations(model, h) = Ok                      │      │
│  │    →  ∃ linearization of h consistent with model      │      │
│  └───────────────────────────────────────────────────────┘      │
│                                                                 │
│  If the checker returns Ok, there must exist a sequential       │
│  permutation that respects real-time order and the model.       │
│                                                                 │
│  Enforced by: DFS correctness in checker.rs                     │
│  Tested by:   prop_soundness in property_tests.rs               │
│  Formal:      Quint `soundness` invariant in Porcupine.qnt      │
└─────────────────────────────────────────────────────────────────┘
```

This is the **only manually written artifact**. Everything below is LLM-generated.

---

## 3. Stage 2: Quint Formal Specification (LLM-Generated)

**File**: `tla/Porcupine.qnt`

The LLM translates the English spec into a machine-checkable Quint model of the DFS
backtracking linearizability algorithm:

```
┌─────────────────────────────────────────────────────────────────┐
│  spec.md                          Porcupine.qnt                 │
│                                                                 │
│  "check = Ok → linearizable"     val soundness: bool =         │
│          │                           result == Ok =>           │
│          │    LLM translates         isLinearizable(history)   │
│          └──────────────────►                                   │
│                                                                 │
│  "history is linearizable iff    val pCompositionality: bool = │
│   each partition is"                 ...                        │
│          └──────────────────►                                   │
│                                                                 │
│  "same (bitset, state) =>        val cacheSound: bool =         │
│   same result"                       ...                        │
│          └──────────────────►                                   │
└─────────────────────────────────────────────────────────────────┘
```

Run with:
```bash
quint verify tla/Porcupine.qnt --invariant safetyInvariant
```

---

## 4. Stage 3: Runtime Invariant Assertions (LLM-Generated)

**File**: `src/invariants.rs`

```
┌─────────────────────────────────────────────────────────────────┐
│  Porcupine.qnt                    src/invariants.rs             │
│                                                                 │
│  val histWellFormed: bool =       macro_rules!                  │
│      ops.forall(op =>                 assert_well_formed {      │
│          op.call <= op.ret)           // INV-HIST-01            │
│                │                      debug_assert!(            │
│                │  LLM translates        op.call <=              │
│                └──────────────►         op.return_time);       │
│                                   }                             │
└─────────────────────────────────────────────────────────────────┘
```

Every `debug_assert!` cites its `INV-*` ID from `spec.md`. The `/verify-invariants`
command checks that no ID exists in one place but not the other.

---

## 5. Stage 4: Property-Based Tests (LLM-Generated)

**File**: `tests/property_tests.rs`

```
┌─────────────────────────────────────────────────────────────────┐
│  spec.md                          property_tests.rs             │
│                                                                 │
│  INV-LIN-01: Soundness            proptest! {                   │
│                                     fn prop_soundness(          │
│          ┌── LLM generates           hist in arb_history()) {  │
│          │                           // If Ok, verify manually  │
│          └──────────────────►        prop_assert!(check(hist)   │
│                                          implies_linearizable); │
│                                   }}                            │
│                                                                 │
│  INV-LIN-03: P-Compositionality   fn prop_compositionality ...  │
│  INV-LIN-04: Cache Soundness      fn prop_cache_sound ...       │
└─────────────────────────────────────────────────────────────────┘
```

Run with:
```bash
cargo test --test property_tests
```

---

## 6. Stage 5: Model-Based Testing via quint-connect (LLM-Generated)

**File**: `tests/quint_mbt.rs` (feature-gated: `--features quint-mbt`)

```
┌─────────────────────────────────────────────────────────────────┐
│  Porcupine.qnt                    quint_mbt.rs                  │
│                                                                 │
│  quint run generates              Rust replays each trace step  │
│  ITF execution traces:            against check_operations():   │
│                                                                 │
│  { "action": "tryLinearize",      let result =                  │
│    "state": { "linearized": …,      check_operations(           │
│               "result": "Ok" } }      &model, &history);        │
│          │                         assert_eq!(result,           │
│          └──────── compare ──────►   expected_from_trace);      │
└─────────────────────────────────────────────────────────────────┘
```

Run with:
```bash
cargo test --features quint-mbt --test quint_mbt
```

---

## 7. Coverage Strategy

Each layer catches different failure modes:

| Layer | What it catches |
|-------|----------------|
| `quint verify` | Logical errors in the algorithm design |
| Property tests | Edge-case histories that violate invariants |
| MBT trace replay | Divergence between Quint model and Rust implementation |
| `debug_assert!` | Runtime violations during development and testing |

---

## 8. Verification Commands (quick reference)

```bash
# Stage 2: Quint model check
quint verify tla/Porcupine.qnt --invariant safetyInvariant

# Stage 3+4: Rust tests (unit + property)
cargo test

# Stage 5: Model-based testing
cargo test --features quint-mbt --test quint_mbt

# Cross-check INV-* IDs between spec.md and invariants.rs
# (invoke /spec-sync skill)

# Full pre-merge suite
# (invoke /verify skill)
```

---

## 9. Failure Taxonomy

| Symptom | Likely root cause | Where to look |
|---------|-------------------|---------------|
| `quint verify` fails | Invariant violated in formal model | `tla/Porcupine.qnt` |
| `prop_soundness` fails | DFS returns Ok for illegal history | `src/checker.rs` |
| `prop_completeness` fails | DFS misses a valid linearization | `src/checker.rs` |
| `prop_compositionality` fails | Partition function splits dependent ops | `src/model.rs` |
| `prop_cache_sound` fails | Cache key collision or state aliasing | `src/checker.rs` (cache) |
| `/spec-sync` reports drift | INV-* added to spec but not code, or vice versa | `docs/spec.md`, `src/invariants.rs` |
| MBT trace mismatch | Rust implementation diverges from Quint model | `src/checker.rs` vs `tla/Porcupine.qnt` |

---

## 10. Why This Pipeline Matters for Agentic Code Generation

When an LLM generates the core algorithm (`checker.rs`), invariants (`invariants.rs`),
and tests (`property_tests.rs`) in one pass, bugs can propagate consistently across all
three — making tests that always agree with wrong code.

The pipeline breaks this coupling:

- `spec.md` is human-authored and cannot be wrong by LLM drift.
- The Quint model is verified independently of the Rust code.
- MBT compares the two worlds directly, catching any divergence.

Even if the LLM generates subtly incorrect Rust, the Quint model will either fail
`quint verify` (catching algorithm-level bugs) or the trace replay will flag the
mismatch (catching implementation-level bugs).
