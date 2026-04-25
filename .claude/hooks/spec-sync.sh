#!/usr/bin/env bash
# Spec-sync: assert each INV-* in docs/spec.md is either asserted in
# src/invariants.rs or marked "(structural)" on its Enforced by line.
#
# Used by:
#   * .claude/settings.local.json (PostToolUse hook on Edit/Write)
#   * .github/workflows/spec-sync.yml (CI gate)
#   * /spec-sync slash command (manual)
#
# Mirrors the pipeline documented in .claude/commands/spec-sync.md so all
# three entry points behave identically.

set -eu

# Locate repo root from the script's own path so the hook works regardless
# of where Claude's cwd happens to be when an edit fires.
script_dir=$(cd "$(dirname "$0")" && pwd)
repo_root=$(cd "$script_dir/../.." && pwd)
spec="$repo_root/docs/spec.md"
code="$repo_root/src/invariants.rs"

if [ ! -f "$spec" ] || [ ! -f "$code" ]; then
  echo "spec-sync: $spec or $code missing — skipping" >&2
  exit 0
fi

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
spec_all=$(printf '%s\n' "$classified" | cut -f1)
code_ids=$(grep -oE 'INV-[A-Z]+-[0-9]+' "$code" | sort -u)

missing=$(comm -23 <(printf '%s\n' "$asserted") <(printf '%s\n' "$code_ids"))
undocumented=$(comm -23 <(printf '%s\n' "$code_ids") <(printf '%s\n' "$spec_all"))

if [ -z "$missing" ] && [ -z "$undocumented" ]; then
  echo "spec-sync: OK"
  exit 0
fi

{
  echo "spec-sync: DRIFT DETECTED between $spec and $code"
  if [ -n "$missing" ]; then
    echo "  missing (asserted in spec, no match in code):"
    printf '%s\n' "$missing" | sed 's/^/    /'
  fi
  if [ -n "$undocumented" ]; then
    echo "  undocumented (in code, no entry in spec):"
    printf '%s\n' "$undocumented" | sed 's/^/    /'
  fi
  echo
  echo "Fix: either add the assertion to src/invariants.rs / spec entry to"
  echo "docs/spec.md, or mark the spec entry's Enforced by line as (structural)."
  echo "See .claude/commands/spec-sync.md for the full policy."
} >&2
exit 1
