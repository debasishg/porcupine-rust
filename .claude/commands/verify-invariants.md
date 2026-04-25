# Invariant Cross-Check

Statically verify that `INV-*` identifiers are consistent between
`docs/spec.md` and `src/invariants.rs`.

This skill applies the policy in `.claude/CLAUDE.md` § *Invariants
Convention*: every spec entry is either **asserted** (has a matching
`debug_assert!` / pub-crate fn in `src/invariants.rs`) or **structural**
(enforced by the algorithm itself, with the spec entry tagged
`(structural)` on its *Enforced by* line). A code assertion without a
matching spec entry is always a violation.

## What this checks

For every `INV-[A-Z]+-[0-9]+` pattern:

1. **Asserted spec entry, present in code** — Matched.
2. **Structural spec entry** — Matched (no code assertion required).
3. **Asserted spec entry, missing from code** — Missing (drift).
4. **Code assertion, no spec entry at all** — Undocumented (drift).

## How to run

This skill is a thin wrapper over `/spec-sync`; both share the same
classification logic. Run that command for the canonical report.

```bash
# Same pipeline as /spec-sync, abbreviated:
classified=$(awk '
  /^### INV-/ {
    if (id != "") emit()
    if (match($0, /INV-[A-Z]+-[0-9]+/)) id = substr($0, RSTART, RLENGTH)
    block = ""
  }
  { block = block "\n" $0 }
  END { if (id != "") emit() }
  function emit() {
    if (block ~ /\(structural\)/) print id "\tstructural"
    else                          print id "\tasserted"
  }
' docs/spec.md | sort -u)

# IDs that must appear in invariants.rs:
printf '%s\n' "$classified" | awk -F'\t' '$2=="asserted"{print $1}'

# IDs actually in invariants.rs:
grep -oE 'INV-[A-Z]+-[0-9]+' src/invariants.rs | sort -u
```

## Output format

| ID | Spec | Code | Status |
|----|------|------|--------|
| INV-HIST-01 | asserted    | ✓ | Matched |
| INV-HIST-02 | structural  | — | Matched (structural) |
| INV-LIN-01  | structural  | — | Matched (structural) |
| INV-LIN-02  | structural  | — | Matched (structural) |
| INV-LIN-03  | asserted    | ✓ | Matched |
| INV-LIN-04  | asserted    | ✓ | Matched |
| INV-ND-01   | structural  | — | Matched (structural) |

Exit non-zero only if a row shows **Missing** or **Undocumented**.

## Retired enforcement names

When a check is renamed or replaced, every reference to the old name must
be removed from `src/invariants.rs`. `/spec-sync` is text-based, so a
lingering comment with the old name will keep matching the ID and mask a
genuine removal.

Names retired so far:

- `assert_partition_independent!` (macro) — superseded by
  `assert_partition_covers_ops` and `assert_partition_events_paired`
  functions, which check disjoint + complete + in-bounds (and Call/Return
  pairing for the events form). INV-LIN-03 enforcement is unchanged in
  scope; only the form changed (macro → `pub(crate) fn`). Refactored in
  commit `2e2fd4a`.

## Why some invariants are structural

Some invariants are whole-algorithm correctness properties — a single-line
`debug_assert!` cannot honestly express them, and a fake assertion would
just re-verify input data without exercising the algorithm. Examples in
this codebase:

- INV-LIN-01 / INV-LIN-02 (soundness / completeness): properties of the
  DFS itself; verified via Quint `resultConsistent` and proptest/Hegel
  sequential-history tests.
- INV-HIST-02 (real-time order): held by construction because the entry
  list is sorted by timestamp before DFS; an assertion would re-verify the
  sort.
- INV-ND-01 (power-set reduction): held by `PowerSetModel::step` fanning
  out across all branches and deduping; verified via Quint
  `powerSetSoundnessInv`.

These are still verified — at the suite level via Quint, proptest, Hegel,
and the MBT trace replay — just not as inline runtime asserts. See
`docs/all_tests.md` § *Invariant Coverage Matrix* for the full picture.
