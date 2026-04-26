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
time. In event-based histories every call event has exactly one corresponding return event,
and every call event precedes its matching return event in the slice (position = time).

- **Enforced by**:
  - `debug_assert!` in `invariants::assert_well_formed` (operation-based histories)
  - `debug_assert!` in `invariants::assert_well_formed_events` (event-based histories)
- **Checked by**:
  - `tests/property_tests.rs` — `prop_well_formed_history`,
    `prop_zero_duration_op_handled` (boundary `call == return_time`),
    `prop_near_u64_max_timestamps_handled` (no overflow),
    `prop_extreme_i64_values_handled`
  - `tests/hegel_properties.rs` — `hegel_well_formed_history`
- **Formal**:  Quint `histWellFormed`

---

### INV-HIST-02: Real-Time Order Preserved

```
∀ op_a, op_b ∈ history:
  op_a.return_time < op_b.call  →  a precedes b in every valid linearization
```

If operation A completes before operation B begins, A must appear before B in any
linearization of the history.

- **Enforced by**: entry ordering in linked-list construction inside `checker.rs` (structural)
- **Checked by**:
  - `tests/property_tests.rs` — `prop_real_time_order`,
    `prop_two_writers_late_reader_matches_membership` (the late read must
    follow both overlapping writes in any valid linearization),
    `prop_time_shift_invariance`,
    `prop_equal_timestamp_tiebreak_invariance`,
    `prop_slice_order_invariance`,
    `prop_all_coincident_timestamps_handled`
  - `tests/hegel_properties.rs` —
    `hegel_two_writers_late_reader_matches_membership`,
    `hegel_time_shift_invariance`,
    `hegel_equal_timestamp_tiebreak_invariance`,
    `hegel_slice_order_invariance`; the `hegel_sequential_*_is_linearizable`
    suites cover this transitively.
- **Formal**: Quint `realTimeOrder`; algebraic counterpart documented as
  `shiftHistory` in `tla/Porcupine.qnt`.

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
  (`tests/property_tests.rs`) and the `hegel_sequential_*_is_linearizable`
  suites in `tests/hegel_properties.rs`
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

- **Enforced by**: correctness of DFS + backtracking in `checker.rs` (structural)
- **Checked by**:
  - `tests/property_tests.rs` — `prop_soundness`,
    `prop_sequential_history_is_linearizable`, `prop_single_op_linearizable`,
    `prop_concurrent_writes_only_is_ok`,
    `prop_concurrent_write_overlap_read_matches_membership`,
    `prop_two_writers_late_reader_matches_membership`,
    `prop_events_agree_with_operations_on_concurrent_history`,
    `prop_client_id_invariance`,
    `prop_always_stutter_nd_history_ok`,
    `prop_no_timeout_never_unknown` (Ok or Illegal — never Unknown
    without a timeout),
    `prop_generous_timeout_matches_unbounded`
  - `tests/hegel_properties.rs` — `hegel_sequential_history_is_linearizable`,
    `hegel_single_op_is_linearizable`, `hegel_empty_history_is_ok`,
    `hegel_prefixes_of_sequential_are_linearizable`,
    `hegel_incremental_register_is_linearizable` (stateful),
    `hegel_concurrent_writes_only_is_ok`,
    `hegel_concurrent_write_overlap_read_matches_membership`,
    `hegel_two_writers_late_reader_matches_membership`,
    `hegel_events_agree_with_operations_on_concurrent_history`,
    `hegel_client_id_invariance`,
    `hegel_always_stutter_nd_history_ok`,
    `hegel_no_timeout_never_unknown`,
    `hegel_generous_timeout_matches_unbounded`,
    `hegel_concurrent_writes_chain_is_linearizable` (stateful)
- **Formal**: Quint `soundness`

---

### INV-LIN-02: Completeness

```
history is linearizable w.r.t. model  →  check_operations(model, history) = Ok
```

If a valid linearization exists, the checker will find it (given sufficient time — no
timeout supplied).

- **Enforced by**: exhaustive DFS in `checker.rs` (structural)
- **Checked by**:
  - `tests/property_tests.rs` — `prop_completeness`,
    `prop_illegal_history_is_detected`,
    `prop_concurrent_write_overlap_read_matches_membership`,
    `prop_two_writers_late_reader_matches_membership`,
    `prop_events_agree_with_operations_on_concurrent_history`,
    `prop_append_preserves_illegal`,
    `prop_always_reject_nd_history_illegal`,
    `prop_adversarial_read_after_writes_is_illegal`,
    `prop_adversarial_kv_read_is_illegal`
  - `tests/hegel_properties.rs` — `hegel_illegal_history_is_detected`,
    `hegel_stale_read_is_always_illegal`,
    `hegel_concurrent_write_overlap_read_matches_membership`,
    `hegel_two_writers_late_reader_matches_membership`,
    `hegel_events_agree_with_operations_on_concurrent_history`,
    `hegel_append_preserves_illegal`,
    `hegel_always_reject_nd_history_illegal`,
    `hegel_adversarial_read_after_writes_is_illegal`,
    `hegel_adversarial_kv_read_is_illegal`
- **Formal**: Quint `completeness`; monotone-extension property
  documented as `appendHistory` in `tla/Porcupine.qnt`.

---

### INV-LIN-03: P-Compositionality

```
∀ partitions P of history:
  (∀ p ∈ P: check_operations(model, p) = Ok)  ↔  check_operations(model, history) = Ok
```

A history is linearizable if and only if each partition produced by `Model::partition`
is independently linearizable. This holds only when the partitioning function produces
truly independent sub-histories (no cross-partition real-time dependencies).

- **Enforced by**: `debug_assert!` in `invariants::assert_partition_covers_ops`
  (operations form) and `invariants::assert_partition_events_paired`
  (events form). Both are debug-only `pub(crate) fn`s; the older
  `assert_partition_independent!` macro was retired in favour of these two
  stronger checks (disjoint + complete + in-bounds, plus call/return pairing
  for the events form).
- **Checked by**:
  - `tests/property_tests.rs` — `prop_compositionality`, plus
    `src/invariants.rs::tests` for the structural cases;
    `prop_kv_partition_equivalence` (per-partition vs whole),
    `prop_kv_partition_order_invariance`,
    `prop_disjoint_keys_independent`,
    `prop_single_key_kv_partition_fast_path`,
    `prop_adversarial_kv_read_is_illegal`
  - `tests/hegel_properties.rs` —
    `hegel_partitions_are_disjoint_and_complete`,
    `hegel_kv_sequential_history_is_linearizable`,
    `hegel_partition_idempotent_with_single_partition`,
    `hegel_kv_partition_equivalence`,
    `hegel_kv_partition_order_invariance`,
    `hegel_single_key_kv_partition_fast_path`,
    `hegel_adversarial_kv_read_is_illegal`,
    `hegel_incremental_kv_is_linearizable` (stateful)
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
- **Checked by**:
  - `tests/property_tests.rs` — `prop_cache_sound`,
    `prop_long_sequential_chain_ok` (Bitset spill past inline capacity)
  - `tests/hegel_properties.rs` — `hegel_cache_sound_deterministic_ops`,
    `hegel_cache_sound_deterministic_events`,
    `hegel_long_sequential_chain_ok`
- **Formal**: Quint `cacheSound`

---

## 3. Nondeterministic Model Invariants

The `NondeterministicModel` trait and `PowerSetModel` adapter introduce one new invariant
governing the correctness of the power-set construction.

### INV-ND-01: Power-Set Reduction Soundness

```
∀ M: NondeterministicModel, ∀ history:
  check_operations(PowerSetModel(M), history) = Ok
    ↔  ∃ sequential linearization of history consistent with M.step
```

The `PowerSetModel` adapter faithfully reduces a `NondeterministicModel` to the
deterministic `Model` interface.  Three structural properties guarantee this:

1. **Empty-set preserving** — If `M.step(s, i, o) = []` for every `s` in the
   current power-state, `PowerSetModel::step` returns `None` (rejection).
2. **Non-empty propagation** — If any `s` in the power-state has at least one
   valid successor, `PowerSetModel::step` returns `Some(successors)`.
3. **Deduplication** — The successor set is deduplicated via `PartialEq`; the
   power-state never contains two states that compare equal.

A degenerate `NondeterministicModel` whose step always returns exactly one
successor is equivalent to the corresponding deterministic `Model`; the two must
agree on all histories.

- **Enforced by**: `PowerSetModel::step` in `src/model.rs` (structural)
- **Checked by**:
  - `tests/property_tests.rs` — `prop_nd_*`,
    `prop_powerset_step_has_no_duplicates` (dedup invariant),
    `prop_powerset_eq_hashed_powerset`,
    `prop_concurrent_lossy_writes_membership`,
    `prop_always_reject_nd_history_illegal`,
    `prop_always_stutter_nd_history_ok`
  - `tests/hegel_properties.rs` — `hegel_nd_deterministic_agrees_with_model`,
    `hegel_nd_sequential_writes_linearizable`,
    `hegel_nd_impossible_read_is_illegal`,
    `hegel_powerset_step_has_no_duplicates`,
    `hegel_powerset_eq_hashed_powerset`,
    `hegel_concurrent_lossy_writes_membership`,
    `hegel_always_reject_nd_history_illegal`,
    `hegel_always_stutter_nd_history_ok`
- **Formal**: `tla/NondeterministicModel.qnt` — `powerSetSoundnessInv`

---

## 4. Invariant Traceability Matrix

The matrix below cites primary tests per invariant; for the full set of
asserts and properties exercising each ID, see the `Checked by` lines in
§§1–3 above. Test files are organised into sub-sections (§1 concurrent
histories, §2 algebraic invariance, §3 partition equivalence, §4
nondeterministic, §5 timeout, §6 edge cases, §7 stateful, §9 round-trip,
§10 negative-control) which are referenced where individual enumeration
would be too verbose.

| ID | spec.md | invariants.rs | property_tests.rs | hegel_properties.rs | Quint |
|----|---------|---------------|-------------------|---------------------|-------|
| INV-HIST-01 | §1 | `assert_well_formed`, `assert_well_formed_events` | `prop_well_formed_history` (+ §6 boundary tests) | `hegel_well_formed_history` | `Porcupine.qnt histWellFormed` |
| INV-HIST-02 | §1 | (entry ordering) | `prop_real_time_order`, `prop_two_writers_late_reader_matches_membership` (+ §2 algebraic, §6 all-coincident) | `hegel_two_writers_late_reader_matches_membership` (+ §2 algebraic, transitive via `hegel_sequential_*`) | `Porcupine.qnt realTimeOrder`, `shiftHistory` |
| INV-HIST-03 | §1 | `assert_minimal_call` | `prop_soundness` | (transitive via `hegel_sequential_*`) | `Porcupine.qnt minimalCallFrontier` |
| INV-LIN-01 | §2 | (DFS correctness) | `prop_soundness`, `prop_sequential_history_is_linearizable`, `prop_single_op_linearizable` (+ §1 concurrent, §2 algebraic, §4 always-stutter, §5 timeout) | `hegel_sequential_history_is_linearizable`, `hegel_single_op_is_linearizable`, `hegel_incremental_register_is_linearizable`, `hegel_concurrent_writes_chain_is_linearizable` (+ §1, §2, §4, §5) | `Porcupine.qnt resultConsistent` |
| INV-LIN-02 | §2 | (DFS exhaustive) | `prop_completeness`, `prop_illegal_history_is_detected` (+ §1 concurrent, §2.5 append, §4.4 always-reject, §10 adversarial) | `hegel_illegal_history_is_detected`, `hegel_stale_read_is_always_illegal` (+ §1, §2.5, §4.4, §10) | `Porcupine.qnt resultConsistent`, `appendHistory` |
| INV-LIN-03 | §2 | `assert_partition_covers_ops`, `assert_partition_events_paired` | `prop_compositionality_*` (+ §3 equivalence/order/disjoint/single-key, §10.2 adversarial KV) | `hegel_partitions_are_disjoint_and_complete`, `hegel_partition_idempotent_with_single_partition`, `hegel_incremental_kv_is_linearizable` (+ §3, §10.2) | `Porcupine.qnt pCompositionality` |
| INV-LIN-04 | §2 | `assert_cache_sound` | `prop_cache_sound`, `prop_long_sequential_chain_ok` (Bitset spill) | `hegel_cache_sound_deterministic_ops`, `hegel_cache_sound_deterministic_events`, `hegel_long_sequential_chain_ok` | `Porcupine.qnt cacheSound` |
| INV-ND-01 | §3 | (structural in `PowerSetModel::step`) | `prop_nd_*` (+ §4 dedup, PowerSet≡HashedPowerSet, lossy concurrent, always-reject/stutter) | `hegel_nd_*` (+ §4 same five families) | `NondeterministicModel.qnt powerSetSoundnessInv` |

> **Parallel execution**: `check_operations` and `check_events` always use rayon to check partitions concurrently (unconditional dependency, no feature flag), matching Go's goroutine-per-partition behaviour.
