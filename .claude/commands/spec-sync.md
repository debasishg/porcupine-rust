# INV-* Spec Sync

Detect drift between `docs/spec.md` (formal invariant definitions) and
`src/invariants.rs` (runtime `debug_assert!` enforcement).

## How it works

Extracts `INV-[A-Z]+-[0-9]+` patterns from both files and classifies each ID:

1. **Matched** — present in both (healthy state).
2. **Spec-only** — in `docs/spec.md` but no corresponding `debug_assert!` in `src/invariants.rs`.
   Action: add the macro to `src/invariants.rs`.
3. **Code-only** — asserted in `src/invariants.rs` but not documented in `docs/spec.md`.
   Action: add the invariant to `docs/spec.md` or remove the undocumented assertion.

## Run

```bash
diff \
  <(grep -oE 'INV-[A-Z]+-[0-9]+' docs/spec.md | sort -u) \
  <(grep -oE 'INV-[A-Z]+-[0-9]+' src/invariants.rs | sort -u)
```

## Output

Print a summary per ID with its classification and the recommended action.
Exit with status 1 if any drift is detected; exit 0 if all IDs are matched.

CI integration: add this as a pre-merge gate alongside `/verify`.
