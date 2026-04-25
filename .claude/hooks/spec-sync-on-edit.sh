#!/usr/bin/env bash
# PostToolUse hook: re-run spec-sync after an Edit/Write that touched
# docs/spec.md or src/invariants.rs.
#
# Wired up in .claude/settings.local.json:
#   hooks.PostToolUse[].matcher  = "Edit|Write|MultiEdit"
#   hooks.PostToolUse[].hooks[].command = ".claude/hooks/spec-sync-on-edit.sh"
#
# The hook receives the tool invocation as JSON on stdin; we extract
# tool_input.file_path and only run the gate when it's relevant. Output
# goes to stderr so Claude sees it and can react.

set -eu

# Read the JSON payload (best-effort: if jq is missing, fall through and
# always run the gate — false-positive cost is just an extra ~50 ms).
payload=$(cat)

if command -v jq >/dev/null 2>&1; then
  file_path=$(printf '%s' "$payload" | jq -r '.tool_input.file_path // empty')
else
  # Fall back to a regex scan; not strictly correct, but good enough for
  # the only two paths we care about.
  file_path=$(printf '%s' "$payload" | grep -oE '"file_path":[ ]*"[^"]+"' | head -1 | sed 's/.*"\([^"]\+\)"$/\1/')
fi

case "$file_path" in
  *docs/spec.md|*src/invariants.rs)
    ;;
  *)
    exit 0
    ;;
esac

script_dir=$(cd "$(dirname "$0")" && pwd)
exec "$script_dir/spec-sync.sh"
