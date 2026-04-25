# INV-* Spec Sync

Detect drift between `docs/spec.md` (formal invariant definitions) and
`src/invariants.rs` (runtime debug-assert enforcement).

## Policy

Per `.claude/CLAUDE.md` § *Invariants Convention*, every spec entry has one
of two valid enforcement forms:

- **Asserted** — has a `debug_assert!` (or `pub(crate) fn` debug-check) in
  `src/invariants.rs` whose message cites the `INV-*` ID.
- **Structural** — enforced by the algorithm's construction. The spec
  entry's *Enforced by* line must include the literal token `(structural)`.

`/spec-sync` flags an ID only when the two artefacts disagree:

| Category | Meaning | Action |
|----------|---------|--------|
| **Matched** | Asserted in spec **and** present in `src/invariants.rs`, OR marked structural in spec | Healthy. |
| **Missing** | Asserted in spec but not found in `src/invariants.rs` | Add the assertion to `src/invariants.rs`, or flip the spec entry to `(structural)` if no honest assertion exists. |
| **Undocumented** | In `src/invariants.rs` but no entry in `docs/spec.md` | Add the entry to `docs/spec.md` or remove the assertion. |

Exit non-zero if any **Missing** or **Undocumented** IDs are reported.

## Run

```bash
spec=docs/spec.md
code=src/invariants.rs

# Classify each spec ID by its section block (header → next "---" or end).
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
' "$spec" | sort -u)

asserted=$(printf '%s\n' "$classified" | awk -F'\t' '$2=="asserted"   {print $1}')
structural=$(printf '%s\n' "$classified" | awk -F'\t' '$2=="structural" {print $1}')
spec_all=$(printf '%s\n' "$classified" | cut -f1)
code_ids=$(grep -oE 'INV-[A-Z]+-[0-9]+' "$code" | sort -u)

missing=$(comm -23 <(printf '%s\n' "$asserted") <(printf '%s\n' "$code_ids"))
undocumented=$(comm -23 <(printf '%s\n' "$code_ids") <(printf '%s\n' "$spec_all"))

# Portable across bash and zsh: pass content as a single newline-joined
# string and indent via sed, so we don't rely on unquoted-expansion word-
# splitting (which differs between shells).
print_block() {
  label=$1
  content=$2
  if [ -z "$content" ]; then
    printf '  %s: (none)\n' "$label"
  else
    printf '  %s:\n' "$label"
    printf '%s\n' "$content" | sed 's/^/    /'
  fi
}

echo "Spec classification:"
print_block "asserted"   "$asserted"
print_block "structural" "$structural"
echo
echo "Cross-check vs $code:"
print_block "missing      (asserted in spec, no match in code)" "$missing"
print_block "undocumented (in code, no entry in spec)"          "$undocumented"

if [ -n "$missing$undocumented" ]; then
  echo; echo "DRIFT DETECTED"; exit 1
else
  echo; echo "OK — all IDs accounted for"
fi
```

## Expected output (current repo)

```
Spec classification:
  asserted:
    INV-HIST-01
    INV-HIST-03
    INV-LIN-03
    INV-LIN-04
  structural:
    INV-HIST-02
    INV-LIN-01
    INV-LIN-02
    INV-ND-01

Cross-check vs src/invariants.rs:
  missing      (asserted in spec, no match in code): (none)
  undocumented (in code, no entry in spec): (none)

OK — all IDs accounted for
```

## Notes for maintainers

- A **stale comment** referencing a retired enforcement name (e.g.
  `assert_partition_independent!`) will satisfy the grep and silently mask a
  removal. Keep `src/invariants.rs` clean of references to retired names —
  see `/verify-invariants` for the running list.
- If a spec entry has *no* `### INV-...` header but the ID appears in prose
  (e.g. cross-references), it will be skipped by classification. Always
  define each ID with its own `### INV-...` section.
- Marking an entry `(structural)` is a deliberate policy choice: the
  invariant cannot honestly be asserted at a single call site (e.g.
  whole-algorithm soundness). Reviewers should push back on `(structural)`
  if a clean point-check exists.

## CI integration

Run as a pre-merge gate alongside `/verify`. The commands here are
shell-portable (POSIX `awk` + `comm`) and require no Rust toolchain.
