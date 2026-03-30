# Porcupine-Rust — Invariant Specification

> **Source of truth** for all `INV-*` identifiers used across this codebase.
> Every `INV-*` ID here must have a matching `debug_assert!` in `src/invariants.rs`.
> Every `debug_assert!` in `src/invariants.rs` must reference an `INV-*` ID here.

---

## 1. History Invariants

### INV-HIST-01: Well-Formed History

```
∀ op ∈ history: op.call ≥ 0 ∧ op.return_time ≥ op.call
∀ event ∈ events: every Call event has exactly one matching Return event with the same id
```

Every operation has non-negative timestamps, and return time is never earlier than call
time. In event-based histories every call event has exactly one corresponding return event.

- **Enforced by**: `debug_assert!` in `invariants::assert_well_formed`
- **Checked by**: `tests/property_tests.rs` — `prop_well_formed_history`
- **Formal**:  Quint `histWellFormed`

---

### INV-HIST-02: Real-Time Order Preserved

```
∀ op_a, op_b ∈ history:
  op_a.return_time < op_b.call  →  a precedes b in every valid linearization
```

If operation A completes before operation B begins, A must appear before B in any
linearization of the history.

- **Enforced by**: entry ordering in linked-list construction inside `checker.rs`
- **Checked by**: `tests/property_tests.rs` — `prop_real_time_order`
- **Formal**: Quint `realTimeOrder`

---

### INV-HIST-03: Minimal-Call Frontier

```
At each DFS step, op is eligible iff ∀ op' ≠ op: op'.call < op.call → op' is already linearized
```

Only operations whose call timestamp is not preceded by any unlinearized call are
candidates for the next linearization step. This ensures the search respects real-time
ordering.

- **Enforced by**: `debug_assert!` in `invariants::assert_minimal_call`
- **Checked by**: implicit in DFS correctness, covered by `prop_soundness`
- **Formal**: Quint `minimalCallFrontier`

---

## 2. Linearizability Invariants

### INV-LIN-01: Soundness

```
check_operations(model, history) = Ok  →  history is linearizable w.r.t. model
```

If the checker returns `Ok`, there must exist a sequential permutation of the operations
that (a) is consistent with real-time order and (b) satisfies the model's step function
at every step.

- **Enforced by**: correctness of DFS + backtracking in `checker.rs`
- **Checked by**: `tests/property_tests.rs` — `prop_soundness`
- **Formal**: Quint `soundness`

---

### INV-LIN-02: Completeness

```
history is linearizable w.r.t. model  →  check_operations(model, history) = Ok
```

If a valid linearization exists, the checker will find it (given sufficient time — no
timeout supplied).

- **Enforced by**: exhaustive DFS in `checker.rs`
- **Checked by**: `tests/property_tests.rs` — `prop_completeness`
- **Formal**: Quint `completeness`

---

### INV-LIN-03: P-Compositionality

```
∀ partitions P of history:
  (∀ p ∈ P: check_operations(model, p) = Ok)  ↔  check_operations(model, history) = Ok
```

A history is linearizable if and only if each partition produced by `Model::partition`
is independently linearizable. This holds only when the partitioning function produces
truly independent sub-histories (no cross-partition real-time dependencies).

- **Enforced by**: `debug_assert!` in `invariants::assert_partition_independent`
- **Checked by**: `tests/property_tests.rs` — `prop_compositionality`
- **Formal**: Quint `pCompositionality`

---

### INV-LIN-04: Cache Soundness

```
∀ (bitset_a, state_a), (bitset_b, state_b):
  bitset_a = bitset_b ∧ state_a = state_b  →  result_a = result_b
```

Two DFS nodes with identical linearized-operation sets and identical model state will
always yield the same sub-tree result. The cache may safely prune any node whose
`(bitset, state)` pair has been seen before.

- **Enforced by**: `debug_assert!` in `invariants::assert_cache_sound`
- **Checked by**: `tests/property_tests.rs` — `prop_cache_sound`
- **Formal**: Quint `cacheSound`

---

## 3. Invariant Traceability Matrix

| ID | spec.md | invariants.rs | property_tests.rs | Porcupine.qnt |
|----|---------|---------------|-------------------|---------------|
| INV-HIST-01 | §1 | `assert_well_formed` | `prop_well_formed_history` | `histWellFormed` |
| INV-HIST-02 | §1 | (entry ordering) | `prop_real_time_order` | `realTimeOrder` |
| INV-HIST-03 | §1 | `assert_minimal_call` | `prop_soundness` | `minimalCallFrontier` |
| INV-LIN-01 | §2 | (DFS correctness) | `prop_soundness` | `soundness` |
| INV-LIN-02 | §2 | (DFS exhaustive) | `prop_completeness` | `completeness` |
| INV-LIN-03 | §2 | `assert_partition_independent` | `prop_compositionality` | `pCompositionality` |
| INV-LIN-04 | §2 | `assert_cache_sound` | `prop_cache_sound` | `cacheSound` |
