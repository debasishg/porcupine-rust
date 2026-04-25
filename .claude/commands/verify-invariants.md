# Invariant Cross-Check

Statically verify that `INV-*` identifiers are consistent between `docs/spec.md` and
`src/invariants.rs`.

## What this checks

For every `INV-[A-Z]+-[0-9]+` pattern:

1. Extract all IDs from `docs/spec.md`.
2. Extract all IDs referenced inside `debug_assert!` calls in `src/invariants.rs`.
3. Report three categories:
   - **Matched** — present in both (compliant).
   - **Spec-only** — documented but no `debug_assert!` (missing runtime enforcement).
   - **Code-only** — asserted in code but not in spec (undocumented assertion).

## How to run

```bash
# Extract from spec.md
grep -oE 'INV-[A-Z]+-[0-9]+' docs/spec.md | sort -u

# Extract from invariants.rs
grep -oE 'INV-[A-Z]+-[0-9]+' src/invariants.rs | sort -u

# Diff them
comm -3 \
  <(grep -oE 'INV-[A-Z]+-[0-9]+' docs/spec.md | sort -u) \
  <(grep -oE 'INV-[A-Z]+-[0-9]+' src/invariants.rs | sort -u)
```

## Output format

| ID | spec.md | invariants.rs | Status |
|----|---------|---------------|--------|
| INV-HIST-01 | ✓ | ✓ | Matched |
| INV-HIST-02 | ✓ | — | Spec-only ⚠ |
| INV-LIN-04 | — | ✓ | Code-only ⚠ |

Exit non-zero if any spec-only or code-only entries are found.

## Note on INV-HIST-02 and INV-LIN-01/02

Some invariants (e.g., INV-HIST-02 real-time ordering, INV-LIN-01/02 soundness/completeness)
are enforced structurally by the algorithm rather than by explicit `debug_assert!` macros.
These are documented in the traceability matrix in `docs/spec.md` and are exempt from the
code-only warning. List the known structural-only IDs here as they are established.

## Retired enforcement names

When a check is renamed or replaced, the old name should disappear from
`src/invariants.rs` entirely — `/spec-sync` is text-based, so a stale comment
will keep matching the ID. Names removed so far:

- `assert_partition_independent!` (macro) — superseded by the
  `assert_partition_covers_ops` and `assert_partition_events_paired`
  functions, which check disjoint + complete + in-bounds (and pairing for the
  events form). INV-LIN-03 enforcement is unchanged in scope; only the form
  changed (macro → `pub(crate) fn`).
