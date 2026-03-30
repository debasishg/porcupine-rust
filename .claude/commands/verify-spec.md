# Formal Specification Verification

Two-phase process verifying the Quint model and its correspondence to the Rust implementation.

## Phase 1 — Quint Model Check

Run the Quint model checker on `tla/Porcupine.qnt`:

```bash
quint verify tla/Porcupine.qnt --invariant safetyInvariant
```

This checks that `safetyInvariant` (which combines `histWellFormedInv`, `minimalCallFrontier`,
`cacheSound`, and `resultConsistent`) holds across all reachable states of the DFS model.

Report:
- Exit status (0 = all invariants hold).
- If invariants are violated, include the counterexample trace from Quint.

## Phase 2 — ITF Trace Replay (Model-Based Testing)

Run the MBT test suite, which generates ITF traces from the Quint model and replays
them against the Rust `check_operations` implementation:

```bash
cargo test --features quint-mbt --test quint_mbt
```

This requires:
- `quint` CLI ≥ 0.31.0 (`npm install -g @informalsystems/quint`)
- `cargo` with the `quint-mbt` feature

Report:
- Exit status of `cargo test`.
- If MBT tests fail, include the failing trace step and the discrepancy between
  the expected Quint result and the actual Rust result.

## Invariants Exercised

- INV-LIN-01 (Soundness): Quint `soundness` + `mbt_trace_matches_rust_checker`
- INV-LIN-02 (Completeness): Quint `completeness` + `mbt_trace_matches_rust_checker`
- INV-HIST-01 (Well-Formed): Quint `histWellFormedInv`
- INV-HIST-03 (Minimal Frontier): Quint `minimalCallFrontier`
- INV-LIN-04 (Cache Sound): Quint `cacheSound`
