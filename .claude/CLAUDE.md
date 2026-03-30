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
- Every `INV-*` ID in `docs/spec.md` must have a matching `debug_assert!` in `src/invariants.rs`.
- Every `debug_assert!` in `src/invariants.rs` must cite its `INV-*` ID.
- Never add an assertion without a spec entry; never add a spec entry without an assertion.
- Run `/spec-sync` to detect drift between the two.

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
| INV-HIST-01 | Well-Formed History | `assert_well_formed!` in checker.rs |
| INV-HIST-02 | Real-Time Order | Entry ordering in linked-list construction |
| INV-HIST-03 | Minimal-Call Frontier | `assert_minimal_call!` in DFS loop |
| INV-LIN-01 | Soundness | DFS correctness + Quint `soundness` |
| INV-LIN-02 | Completeness | DFS exhaustiveness + Quint `completeness` |
| INV-LIN-03 | P-Compositionality | `assert_partition_independent!` + proptest |
| INV-LIN-04 | Cache Soundness | `assert_cache_sound!` + Quint `cacheSound` |

Full definitions: `docs/spec.md`. Traceability matrix: `docs/spec.md §3`.
