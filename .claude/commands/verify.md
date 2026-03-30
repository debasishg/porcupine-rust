# Pre-Merge Verification Suite

Orchestrates all mandatory verification stages. Run this before every merge.

## Process

Execute the following stages **in order**, halting on first failure:

**Stage 1 — Specification Verification** (`/verify-spec`)
Run the Quint model checker and replay ITF traces against the Rust implementation.
Verifies that the formal model is consistent and that Rust matches it.

**Stage 2 — Invariant Cross-Check** (`/verify-invariants`)
Statically verify that every `INV-*` ID in `docs/spec.md` has a matching `debug_assert!`
in `src/invariants.rs`, and vice versa.

## Output

After all stages pass, print a summary table:

| Stage | Status |
|-------|--------|
| /verify-spec | ✓ Pass |
| /verify-invariants | ✓ Pass |

If any stage fails, halt immediately and print the full output of the failing stage.
