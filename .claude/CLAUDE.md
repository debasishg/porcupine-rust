# porcupine-rust

A Rust port of [porcupine](https://github.com/anishathalye/porcupine), a fast linearizability checker for concurrent systems.

## Project Overview

This library checks whether a concurrent operation history is linearizable with respect to a sequential specification model. It is used to verify correctness of distributed systems and concurrent data structures.

## Build & Test

```bash
cargo build
cargo test
cargo clippy
```

## Code Style

- Follow standard Rust idioms and the Rust API Guidelines.
- Run `cargo clippy` and resolve all warnings before committing.
- Run `cargo fmt` to format code.
- Prefer `thiserror` for error types and `std` traits over custom ones where possible.
- Use `#[derive(Debug, Clone, PartialEq)]` on data types where applicable.

## Architecture

- `src/lib.rs` — public API and re-exports
- `src/model.rs` — `Model` trait and related types
- `src/checker.rs` — core linearizability checking algorithm (DFS + backtracking + caching)
- `src/invariants.rs` — `debug_assert!` macros keyed to `INV-*` IDs from `docs/spec.md`
- `src/types.rs` — `Operation`, `Event`, `CheckResult`, `LinearizationInfo`

## Key Concepts

- **Model**: A sequential specification — init state + step function + optional partition function.
- **Operation**: A completed concurrent operation with call/return timestamps.
- **Event**: A raw call or return event (alternative history representation).
- **CheckResult**: `Ok` (linearizable), `Illegal` (not linearizable), or `Unknown` (timeout).
- **Linearizability**: A history is linearizable if the concurrent operations can be reordered to a valid sequential execution consistent with real-time ordering.

---

## Self-Verified Pipeline

This project uses a five-stage invariant validation pipeline. See `docs/self-verified-pipeline.md`
for the full visual walkthrough. All model checking is driven by Quint.

```
docs/spec.md  →  tla/Porcupine.qnt  →  src/invariants.rs (debug_assert!)
                                    →  tests/property_tests.rs (proptest)
                                    →  tests/quint_mbt.rs (trace replay)
```

### Invariants Convention

- `docs/spec.md` is the **single source of truth** for all `INV-*` identifiers.
- Each spec entry must have an **Enforced by** line classifying how the
  invariant is upheld. Two valid forms:
  - **Asserted**: a `debug_assert!` (or `pub(crate) fn` debug-check) in
    `src/invariants.rs`. The assertion message must cite the `INV-*` ID.
  - **Structural**: enforced by the algorithm's construction (e.g. sort
    order, exhaustive DFS, power-set fan-out). The spec entry must say
    `(structural)` and name the enforcing site (file + concept), so the
    rationale is reviewable.
- Every assertion in `src/invariants.rs` must cite an `INV-*` ID that exists
  in `docs/spec.md`. Never add an assertion without a spec entry.
- Never add a spec entry without either an assertion or an explicit
  `(structural)` enforcement note.
- Run `/spec-sync` to detect drift. The skill flags an ID only when it is
  *neither* asserted *nor* marked structural in the spec.

#### Why "structural" exists as a category

Some invariants are whole-algorithm correctness properties — they cannot
honestly be expressed as a single-line `debug_assert!`. Examples:

- INV-LIN-01 (Soundness): "the DFS result is a valid linearization"
  — a property of the search itself, not a check at any one call site.
- INV-LIN-02 (Completeness): "if a linearization exists, the DFS finds it"
  — same reason; the assertion would be circular.
- INV-HIST-02 (Real-Time Order): holds by construction because the entry
  list is sorted by timestamp; an assertion would just re-verify the sort.
- INV-ND-01 (Power-Set Soundness): enforced by `PowerSetModel::step`
  fanning out across all branches and deduping; not a point-check.

These are still verified — by Quint (`Porcupine.qnt resultConsistent`,
`pCompositionality`, `NondeterministicModel.qnt powerSetSoundnessInv`),
proptest, Hegel, and the MBT trace replay — but at the suite level rather
than as inline runtime asserts.

### Verification Commands

```bash
# Quint model check (requires quint ≥ 0.31.0)
quint verify tla/Porcupine.qnt --invariant safetyInvariant

# Property-based tests
cargo test --test property_tests

# Model-based tests (requires quint CLI + quint-mbt feature)
cargo test --features quint-mbt --test quint_mbt

# Full pre-merge suite
# (invoke /verify skill)
```

### Skills (Slash Commands)

See `SKILLS.md` at the project root for the full hierarchy. Quick reference:

| Command | Purpose |
|---------|---------|
| `/verify` | Full pre-merge suite (orchestrates the two below) |
| `/verify-spec` | Quint model check + ITF trace replay |
| `/verify-invariants` | spec.md ↔ invariants.rs INV-* cross-check |
| `/test-crate` | All tests, all feature combinations |
| `/spec-sync` | Diff INV-* IDs between spec.md and invariants.rs |

### Key Invariants (summary)

| ID | Name | Enforced by |
|----|------|-------------|
| INV-HIST-01 | Well-Formed History | `assert_well_formed!` / `assert_well_formed_events!` (asserted) |
| INV-HIST-02 | Real-Time Order | Entry ordering in linked-list construction (structural) |
| INV-HIST-03 | Minimal-Call Frontier | `assert_minimal_call!` in DFS loop (asserted) |
| INV-LIN-01 | Soundness | DFS correctness + Quint `resultConsistent` (structural) |
| INV-LIN-02 | Completeness | DFS exhaustiveness + Quint `resultConsistent` (structural) |
| INV-LIN-03 | P-Compositionality | `assert_partition_covers_ops` / `assert_partition_events_paired` + proptest (asserted) |
| INV-LIN-04 | Cache Soundness | `assert_cache_sound!` + Quint `cacheSound` (asserted) |
| INV-ND-01 | Power-Set Reduction Soundness | `PowerSetModel::step` in `src/model.rs` + Quint `powerSetSoundnessInv` (structural) |

Full definitions: `docs/spec.md`. Traceability matrix: `docs/spec.md §3`.
