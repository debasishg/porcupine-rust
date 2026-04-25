# `spec-sync`: keeping the spec and the runtime in lockstep

This document explains how this project keeps `docs/spec.md` (the formal
invariant specification) and `src/invariants.rs` (the runtime debug-check
implementation) from drifting apart, and how that consistency is enforced
automatically by three different mechanisms working together.

It's written for someone who has never seen this project before and is
trying to figure out:

- What does *spec-sync* mean?
- Where do the rules live?
- What runs when, and what happens if a check fails?
- How do I add or remove an invariant without breaking anything?

If you only need a quick reminder later, jump to the [Quick reference]
table at the bottom.

---

## 1. The problem this solves

This project verifies that concurrent histories are *linearizable* — a
non-trivial correctness property for distributed systems. To make that
verification trustworthy, the project documents a small list of formal
invariants in `docs/spec.md`. Each invariant has a stable identifier of
the form `INV-<DOMAIN>-<NUMBER>`, for example:

- `INV-HIST-01` — Well-Formed History
- `INV-LIN-03` — P-Compositionality
- `INV-ND-01` — Power-Set Reduction Soundness

Some of those invariants are also enforced *at runtime* by debug-only
assertions in `src/invariants.rs`. For example, `INV-HIST-01` says every
operation must have `call ≤ return_time`, and the matching code in
`src/invariants.rs` looks like this:

```rust
debug_assert!(
    op.call <= op.return_time,
    "INV-HIST-01: op {} has call ({}) > return_time ({})",
    i, op.call, op.return_time,
);
```

So we have two artefacts that are supposed to agree:

| Artefact | Type | Role |
|----------|------|------|
| `docs/spec.md` | Markdown prose | Source of truth — defines each invariant in English plus formal Quint references |
| `src/invariants.rs` | Rust code | Runtime enforcement — `debug_assert!` calls that fail when the property is violated |

**Drift** is what happens when these two get out of sync. A few real
examples:

- Someone adds a new `INV-FOO-99` to the spec but forgets to add the
  matching `debug_assert!`. Production runs without runtime protection.
- Someone refactors `src/invariants.rs` and renames a check, but the spec
  still references the old name. Future readers think a check exists when
  it doesn't.
- Someone adds a `debug_assert!` mentioning `INV-BAR-42`, but no spec
  entry exists for that ID. The assertion fires occasionally and nobody
  knows what it's protecting against.

`spec-sync` is the gate that catches all three.

---

## 2. The policy: asserted vs structural

Not every invariant *can* be expressed as a single-line `debug_assert!`.
Some are properties of the algorithm itself — for example,
"the DFS finds every valid linearization." There's no single point in the
code where you could write `debug_assert!(dfs_is_complete)`; the
invariant *is* the algorithm.

So the policy admits two valid enforcement forms:

### 2.1 Asserted

The invariant is a local check that can fire when violated. It has a
`debug_assert!` (or `pub(crate) fn` debug-check) in `src/invariants.rs`,
and the assertion message cites the `INV-*` ID.

Example — `INV-HIST-01` (well-formed history):

```rust
// src/invariants.rs
pub(crate) fn assert_well_formed<I, O>(history: &[Operation<I, O>]) {
    for (i, op) in history.iter().enumerate() {
        debug_assert!(
            op.call <= op.return_time,
            "INV-HIST-01: op {} has call ({}) > return_time ({})",
            i, op.call, op.return_time,
        );
    }
}
```

### 2.2 Structural

The invariant is a property of the algorithm's construction, not a check
at any single line. The spec entry **must** include the literal token
`(structural)` on its `Enforced by` line, naming the file and the
mechanism that upholds it.

Example — `INV-LIN-01` (soundness):

```markdown
### INV-LIN-01: Soundness

If the checker returns `Ok`, there must exist a sequential permutation
of the operations that …

- **Enforced by**: correctness of DFS + backtracking in `checker.rs` (structural)
- **Checked by**: `tests/property_tests.rs` — `prop_soundness`
- **Formal**: Quint `soundness`
```

Why allow this? Because trying to fake-assert a whole-algorithm property
is worse than admitting it. A `debug_assert!(dfs_returns_only_valid_results)`
that just compares the result against itself adds no protection but
*looks* like it does — that's negative value.

Structural invariants are still **verified**, just not as inline asserts.
They're verified by:

- Quint formal model checks (e.g. `Porcupine.qnt resultConsistent`)
- Property tests (proptest + Hegel) on thousands of random histories
- Model-based tests (MBT) that replay Quint traces through the Rust code

See `docs/spec.md` and `docs/all_tests.md` for the full traceability
matrix.

### 2.3 The hard rules

Three rules govern the policy:

1. Every `INV-*` ID in `docs/spec.md` must be **either** asserted in
   `src/invariants.rs` **or** marked `(structural)` on its `Enforced by`
   line.
2. Every `INV-*` ID referenced in `src/invariants.rs` must have a
   matching spec entry in `docs/spec.md`.
3. The `(structural)` token is reserved — don't use it casually. If a
   clean point-check exists, write it.

If you violate (1) or (2), `spec-sync` fails. If you violate (3), code
review should push back.

---

## 3. The detection mechanism — `spec-sync.sh`

A single bash script does all the work. It lives at:

```
.claude/hooks/spec-sync.sh
```

The script has three stages:

### 3.1 Stage 1 — classify spec entries

Walk `docs/spec.md`, group lines by `### INV-` headers, and for each
group emit either `<id>\tasserted` or `<id>\tstructural` based on whether
the section block contains the literal `(structural)` token.

```bash
awk '
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
' docs/spec.md
```

Output today:

```
INV-HIST-01    asserted
INV-HIST-02    structural
INV-HIST-03    asserted
INV-LIN-01     structural
INV-LIN-02     structural
INV-LIN-03     asserted
INV-LIN-04     asserted
INV-ND-01      structural
```

### 3.2 Stage 2 — list code IDs

Plain grep over `src/invariants.rs`:

```bash
grep -oE 'INV-[A-Z]+-[0-9]+' src/invariants.rs | sort -u
```

Output today:

```
INV-HIST-01
INV-HIST-03
INV-LIN-03
INV-LIN-04
```

### 3.3 Stage 3 — diff the two

Two `comm` calls produce two diff sets:

- **Missing**: IDs that are *asserted* in the spec but *not* in the code.
  This means someone wrote a spec entry without the runtime check (or
  forgot to mark it `(structural)`).

- **Undocumented**: IDs that appear in the code but have *no* spec entry
  at all. This means someone added a `debug_assert!` referencing an
  invariant that nobody documented.

Note that *structural* spec entries are deliberately excluded from the
"missing" check — that's the whole point of the structural classification.

Exit code:

- `0` — no drift, both sets are empty.
- `1` — drift detected; the script prints a report to stderr and exits
  non-zero so callers (CI, hooks, shells) can react.

---

## 4. Three places spec-sync runs

The same script is invoked from three different places, each catching a
different failure class. This is *defense in depth* — none of the three
is sufficient on its own, but together they make drift very hard to
introduce.

### 4.1 Manual — `/spec-sync` slash command

The skill at `.claude/commands/spec-sync.md` is what runs when you type
`/spec-sync` in Claude Code. It's a Markdown file that describes the
policy and embeds the bash recipe. Useful when you want to verify on
demand — for example, before opening a PR.

### 4.2 Local — Claude Code `PostToolUse` hook

After Claude Code performs an `Edit`, `Write`, or `MultiEdit` tool call,
a hook fires and runs the gate **locally**. This gives instant feedback
during a coding session: if Claude (or you, via Claude) edits one of the
two watched files in a way that introduces drift, the report appears in
the session before the change is even committed.

The hook's wrapper script (`spec-sync-on-edit.sh`) filters by file path
so the gate only runs for edits that touch `docs/spec.md` or
`src/invariants.rs` — every other edit is a fast no-op.

### 4.3 Remote — GitHub Actions CI

A workflow runs the gate on every push to `main` and every pull request
that touches the relevant files. This is the **real gate**: it runs
regardless of who or what made the change, and a failing build prevents
merging.

Why all three? Each closes a different gap:

| Path | Catches | Misses |
|------|---------|--------|
| Manual slash command | Whatever you remember to run | Anything you forget |
| Claude Code hook | Edits Claude makes during a session | Edits you make outside Claude |
| GitHub Actions CI | Anything pushed to GitHub | Nothing — this is the safety net |

The slash command and hook give you fast in-loop feedback so you don't
push known-broken state. CI catches anything that slips past those two.

---

## 5. The files — what each one is for

There are seven files involved. Here's a complete inventory:

| File | What it is | Who reads it |
|------|------------|--------------|
| `docs/spec.md` | Source of truth — defines each `INV-*` invariant in prose plus formal references | Humans, the `spec-sync.sh` parser |
| `src/invariants.rs` | Rust source containing `debug_assert!` calls and the four pub-crate functions that cite `INV-*` IDs | Rust compiler, the `spec-sync.sh` parser |
| `.claude/CLAUDE.md` | Project instructions for Claude Code, including the *Invariants Convention* policy | Claude Code |
| `.claude/commands/spec-sync.md` | Slash command definition for `/spec-sync` | Claude Code (when you type the command) |
| `.claude/hooks/spec-sync.sh` | The actual enforcement script — awk pipeline + diff + exit code | All three callers (slash command, hook, CI) |
| `.claude/hooks/spec-sync-on-edit.sh` | Hook wrapper — reads tool JSON from stdin, decides whether to call `spec-sync.sh` | Claude Code's hook runtime |
| `.claude/settings.json` | Project settings registering the `PostToolUse` hook | Claude Code at session start |
| `.github/workflows/spec-sync.yml` | GitHub Actions workflow that runs the gate in CI | GitHub on push and PR events |

Of those, the four files unique to this story (created by the spec-sync
work) are:

- `.claude/hooks/spec-sync.sh`
- `.claude/hooks/spec-sync-on-edit.sh`
- `.claude/settings.json`
- `.github/workflows/spec-sync.yml`

The first two are committed to git, are executable, and live under
`.claude/` so the directory layout matches Claude Code's convention for
project-level customisations.

### 5.1 `spec-sync.sh` in detail

```bash
#!/usr/bin/env bash
set -eu

script_dir=$(cd "$(dirname "$0")" && pwd)
repo_root=$(cd "$script_dir/../.." && pwd)
spec="$repo_root/docs/spec.md"
code="$repo_root/src/invariants.rs"
```

The script resolves the repo root from its own location instead of
assuming the caller's `cwd`. That means it works whether you invoke it
from the repo root, from a deeper subdirectory, or from a Claude Code
hook (which doesn't always run with `cwd` at the repo root).

The rest of the script is the awk + comm pipeline described in §3, plus
formatting that produces output like this on success:

```
spec-sync: OK
```

…and like this on failure (showing real example output for a
hypothetical regression):

```
spec-sync: DRIFT DETECTED between docs/spec.md and src/invariants.rs
  missing (asserted in spec, no match in code):
    INV-FOO-99
  undocumented (in code, no entry in spec):
    INV-BAR-42

Fix: either add the assertion to src/invariants.rs / spec entry to
docs/spec.md, or mark the spec entry's Enforced by line as (structural).
See .claude/commands/spec-sync.md for the full policy.
```

### 5.2 `spec-sync-on-edit.sh` in detail

```bash
#!/usr/bin/env bash
set -eu

payload=$(cat)

if command -v jq >/dev/null 2>&1; then
  file_path=$(printf '%s' "$payload" | jq -r '.tool_input.file_path // empty')
else
  file_path=$(printf '%s' "$payload" | grep -oE '"file_path":[ ]*"[^"]+"' | head -1 \
    | sed 's/.*"\([^"]\+\)"$/\1/')
fi

case "$file_path" in
  *docs/spec.md|*src/invariants.rs) ;;
  *) exit 0 ;;
esac

script_dir=$(cd "$(dirname "$0")" && pwd)
exec "$script_dir/spec-sync.sh"
```

When Claude Code calls a `type: command` hook, the tool invocation is
piped to the command's stdin as JSON. The wrapper:

1. Reads the JSON payload (`payload=$(cat)`).
2. Extracts `tool_input.file_path` — using `jq` if available, falling
   back to a regex scan if not. The regex fallback is intentionally
   simple; it's good enough for the only two paths we care about.
3. If the path doesn't match `docs/spec.md` or `src/invariants.rs`,
   exits 0 immediately. Most edits hit this branch and finish in
   milliseconds.
4. Otherwise, `exec`s `spec-sync.sh` so the wrapper's process is
   replaced — exit code propagates directly.

The path matching is a glob (`*docs/spec.md`, `*src/invariants.rs`) so
it works whether Claude Code passes an absolute path
(`/Users/.../docs/spec.md`), a path relative to the repo
(`docs/spec.md`), or anything in between.

### 5.3 `.claude/settings.json` in detail

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [
          {
            "type": "command",
            "command": "bash .claude/hooks/spec-sync-on-edit.sh",
            "statusMessage": "spec-sync"
          }
        ]
      }
    ]
  }
}
```

A few subtle points:

- The file is `settings.json`, **not** `settings.local.json`. The two
  files are merged at session start, but `settings.local.json` is
  globally gitignored (it's where personal overrides live). For a
  team-wide gate we want the config committed, hence `settings.json`.
- The matcher uses regex alternation: `Edit|Write|MultiEdit` matches all
  three Claude tool names that can change file contents. `Read`, `Bash`,
  `Glob` etc. don't match.
- The `command` invokes `bash` explicitly rather than relying on the
  shebang and the executable bit. This is more portable across systems
  that might ship the script with the executable bit unset.
- `statusMessage` is what Claude Code displays in its spinner while the
  hook runs. If the hook is silent (the typical case), the user never
  sees this message.

### 5.4 `.github/workflows/spec-sync.yml` in detail

```yaml
name: spec-sync

on:
  push:
    branches: [main]
    paths:
      - "docs/spec.md"
      - "src/invariants.rs"
      - ".claude/hooks/spec-sync.sh"
      - ".github/workflows/spec-sync.yml"
  pull_request:
    paths:
      - "docs/spec.md"
      - "src/invariants.rs"
      - ".claude/hooks/spec-sync.sh"
      - ".github/workflows/spec-sync.yml"

jobs:
  spec-sync:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Run spec-sync
        run: bash .claude/hooks/spec-sync.sh
```

Notes:

- The `paths` filter ensures the workflow only spins up when one of the
  four sensitive files changes. A PR that only edits `README.md` skips
  spec-sync entirely — saves CI minutes and avoids noisy "skipped" runs.
- The runner is `ubuntu-latest` because that's free for public repos and
  ships with `bash`, `awk`, `comm`, `grep`, and `sed` already installed.
  No additional setup steps are needed.
- The job has a single shell step. The fewer moving parts in CI, the
  fewer ways the gate itself can break.

---

## 6. Worked examples

These walkthroughs show what each tool reports under different
scenarios. The shell output is *real* — copy-pasted from runs in this
repo.

### 6.1 Clean state — all eight INVs accounted for

```bash
$ bash .claude/hooks/spec-sync.sh
spec-sync: OK
$ echo $?
0
```

That's the entire success path. CI shows a green check, the Claude hook
is silent, the slash command renders a "matched" report.

### 6.2 Manual `/spec-sync` — verbose report

If you run the slash command in Claude Code, you get the full
classification table even on success:

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

### 6.3 Drift scenario — spec entry without assertion

Suppose someone adds this to `docs/spec.md`:

```markdown
### INV-LIN-05: Some New Property

Some explanation here.

- **Enforced by**: `debug_assert!` in `invariants::assert_new_thing`
- **Checked by**: `tests/property_tests.rs` — `prop_new_thing`
```

…but forgets to add `assert_new_thing` to `src/invariants.rs`. On the
next `git commit` followed by `git push`, GitHub Actions runs
`spec-sync` and the build fails with:

```
spec-sync: DRIFT DETECTED between docs/spec.md and src/invariants.rs
  missing (asserted in spec, no match in code):
    INV-LIN-05

Fix: either add the assertion to src/invariants.rs / spec entry to
docs/spec.md, or mark the spec entry's Enforced by line as (structural).
```

Two valid fixes:

- Add `assert_new_thing` to `src/invariants.rs` and reference
  `INV-LIN-05` in the assertion message.
- If no clean assertion exists, change the spec line to
  `**Enforced by**: ... (structural)`.

If you'd been editing in Claude Code, the local hook would have caught
this before the commit even happened — same report, same exit code,
appearing inline in the session.

### 6.4 Drift scenario — code assertion without spec entry

Suppose someone adds this to `src/invariants.rs`:

```rust
debug_assert!(
    !history.is_empty(),
    "INV-FOO-01: history must be non-empty",
);
```

…without adding a corresponding `### INV-FOO-01` section to
`docs/spec.md`. CI fails with:

```
spec-sync: DRIFT DETECTED between docs/spec.md and src/invariants.rs
  undocumented (in code, no entry in spec):
    INV-FOO-01
```

Fix: add a section to `docs/spec.md` describing the property, citing the
test that exercises it, and marking it asserted (or structural, if it
belongs in that category).

### 6.5 The structural escape hatch

Suppose you want to add `INV-PERF-01: Cache hit rate ≥ 80%` — clearly
not something any single `debug_assert!` can express. The right form is:

```markdown
### INV-PERF-01: Cache hit rate threshold

Under any partition workload, cache hit rate must stay ≥ 80% to keep
DFS pruning effective.

- **Enforced by**: cache key shape in `checker.rs` (structural)
- **Checked by**: `benches/linearizability.rs` — runs warn if
  hit rate dips
```

`spec-sync` accepts this and exits 0. The `(structural)` token is the
escape hatch; verification responsibility moves to the bench-suite
mentioned in `Checked by`.

### 6.6 Renaming an asserted check (the stale-comment trap)

Suppose you rename `assert_well_formed` to `assert_history_well_formed`
in `src/invariants.rs` but leave a doc comment referencing the old name:

```rust
// Old: see assert_well_formed for INV-HIST-01 enforcement.
pub(crate) fn assert_history_well_formed(/* ... */) { /* ... */ }
```

`spec-sync` is text-based and only looks for `INV-*` IDs — it doesn't
notice the rename. The comment is fine to keep as long as it references
`INV-HIST-01` correctly. But if the comment says
`// Old: assert_partition_independent! enforced this`, that stale
reference can mask a *real* removal: deleting an assertion but leaving
a comment that still mentions its INV-* ID will keep `spec-sync` happy
even though the assertion is gone.

The fix: when retiring a check, scrub all references — code and comments
— from `src/invariants.rs`. Then update `.claude/commands/verify-invariants.md`
to record the retirement (there's a "Retired enforcement names" section
that lists prior renames).

---

## 7. How this all came together (history)

The spec/invariants split exists because some properties are honestly
algorithmic (and shouldn't be faked as assertions), but the *literal*
reading of the original CLAUDE.md policy was "every spec entry must have
a `debug_assert!`." That policy was never actually true: from the very
first commit (`de53826`, "Add self-verified pipeline"), four invariants
(`INV-HIST-02`, `INV-LIN-01`, `INV-LIN-02`, and later `INV-ND-01`) were
intentionally enforced structurally. The spec said so plainly, in prose,
on each "Enforced by" line. But the original `/spec-sync` was a literal
diff that didn't read those lines, so it would have flagged drift
forever if anyone had run it.

It got noticed during the Hegel test-suite work in April 2026, when
running `/spec-sync` for the first time after months returned four
"spec-only" entries. The fix was to:

1. Acknowledge the mismatch between policy (CLAUDE.md) and practice
   (spec.md).
2. Update CLAUDE.md to admit two enforcement forms.
3. Add `(structural)` markers to the three implicit spec entries (the
   fourth, `INV-ND-01`, already had one).
4. Rewrite `/spec-sync` so its parser actually understands the
   classification.
5. Wire the fixed gate into both Claude Code (for fast feedback) and
   GitHub Actions (for the real merge gate).

The whole reconciliation lives in commits `1f6ec25` (policy change) and
`326fc4e` (automation).

---

## 8. Maintenance — how to add or change an invariant

### 8.1 Adding a new asserted invariant

1. Pick the next free ID in the appropriate domain (e.g. if `INV-LIN-04`
   is the highest LIN-* in the spec, the new one is `INV-LIN-05`).
2. Write the spec entry in `docs/spec.md`. Pattern:
   ```markdown
   ### INV-LIN-05: Title Here

   Plain-English description of the property, with formal notation if
   useful.

   - **Enforced by**: `debug_assert!` in `invariants::assert_my_thing`
   - **Checked by**: `tests/property_tests.rs` — `prop_my_thing`
   - **Formal**: Quint `myThing` (if applicable)
   ```
3. Add the assertion to `src/invariants.rs`. The message **must** start
   with `"INV-LIN-05: "` so the grep in `spec-sync.sh` matches.
4. Wire a call to it from `src/checker.rs` at the appropriate point.
5. Run `bash .claude/hooks/spec-sync.sh` locally to confirm no drift.

### 8.2 Adding a new structural invariant

1. Pick the next free ID.
2. Write the spec entry, but mark `Enforced by` as structural:
   ```markdown
   - **Enforced by**: <where and how the algorithm guarantees this> (structural)
   ```
3. **Don't** add anything to `src/invariants.rs`. The whole point is
   that this invariant has no point-check.
4. Make sure the `Checked by` line cites a real test (Quint, proptest,
   or Hegel) that exercises the property end-to-end.
5. Run the gate to confirm.

### 8.3 Retiring an invariant

1. Remove the assertion from `src/invariants.rs`.
2. Scrub every reference — comments, doc strings, test descriptions —
   that mention the old `INV-*` ID. (If you leave one in `invariants.rs`,
   the gate will think the invariant still exists.)
3. Either delete the spec entry or, if it should remain documented as
   historical context, mark it explicitly retired (e.g. add a
   `> **Retired in commit X.**` blockquote and convert the `Enforced by`
   line to something like `(retired)`). The current `/spec-sync` does
   *not* recognise a retired marker — it will still flag the entry as
   needing an assertion. So usually the cleanest option is full deletion
   plus a note in `verify-invariants.md`'s retirement log.

### 8.4 Renaming an assertion

The grep-based gate doesn't care what an assertion's *function* is
called — only what `INV-*` ID its message cites. So renaming
`assert_old_name` to `assert_new_name` is a free operation as long as:

- The assertion message still cites the same `INV-*` ID.
- No comment elsewhere claims the old name still exists.
- The spec entry's `Enforced by` line is updated to reference the new
  name (purely cosmetic — the gate doesn't read it).

This was the situation that produced commit `2e2fd4a`: the partition
checks were converted from macros to `pub(crate) fn`s, but each one
still cited `INV-LIN-03` in its panic message, so the gate stayed green.

---

## 9. Limitations and known caveats

**1. The Claude Code hook needs one restart after install.**
Claude Code's settings watcher only watches `.claude/` directories that
contained a settings file when the session started. If you clone the
repo fresh and immediately edit `docs/spec.md` in the same session, the
hook may not fire — the watcher hasn't seen the file yet. Open the
`/hooks` menu (which forces a config reload) or restart Claude Code, and
the hook will be live in every session afterwards. CI is unaffected.

**2. Linux/macOS only.**
Both the script and the regex fallback assume POSIX shell tools (`bash`,
`awk`, `comm`, `sed`, `grep`). They have not been tested on Windows. If
you need Windows support, run under WSL or a Git Bash environment.

**3. The grep is text-based.**
`spec-sync` cannot tell the difference between a real assertion and a
comment that happens to mention `INV-*-NN`. If you write
`// TODO: implement INV-FOO-01` in `src/invariants.rs`, the gate will
think `INV-FOO-01` exists in code. This is rarely a problem in practice
because `INV-*` IDs are only ever introduced deliberately, but it does
mean stale comments after a removal can mask drift. See §6.6.

**4. `jq` is optional but recommended.**
The wrapper script falls back to a regex scan when `jq` is not
installed. The fallback is fine for the two file paths we watch but
would be brittle for more complex queries. macOS ships with `jq` only
if you install it (`brew install jq`); GitHub Actions runners have it
pre-installed.

**5. Structural is an escape hatch, not a default.**
If every new invariant gets `(structural)` slapped on it, the whole
system erodes into "documentation that nothing enforces." The token
should be reserved for cases where a clean point-check genuinely doesn't
exist. Code review is the human check on this.

---

## 10. Quick reference

### Files

```
docs/spec.md                            ← source of truth
src/invariants.rs                       ← runtime asserts
.claude/CLAUDE.md                       ← Invariants Convention policy
.claude/commands/spec-sync.md           ← /spec-sync slash command
.claude/commands/verify-invariants.md   ← cross-check skill (related)
.claude/hooks/spec-sync.sh              ← THE script
.claude/hooks/spec-sync-on-edit.sh      ← Claude hook wrapper
.claude/settings.json                   ← registers the hook
.github/workflows/spec-sync.yml         ← CI gate
```

### Commands you'll actually run

```bash
# Run the gate locally:
bash .claude/hooks/spec-sync.sh

# Or, if in Claude Code:
/spec-sync

# Inspect classification in detail:
awk '/^### INV-/{ if(id) p(); match($0,/INV-[A-Z]+-[0-9]+/); id=substr($0,RSTART,RLENGTH); b="" } { b=b"\n"$0 } END{ if(id) p() } function p(){ if(b ~ /\(structural\)/) print id, "structural"; else print id, "asserted" }' docs/spec.md
```

### The single cheat sheet

| Symptom | Likely cause | Fix |
|---------|-------------|-----|
| CI fails: `missing INV-X-Y` | Spec says asserted, no debug_assert | Add the assert, **or** mark `(structural)` |
| CI fails: `undocumented INV-X-Y` | Code asserts, no spec entry | Add spec section in `docs/spec.md` |
| Hook is silent on relevant edits | Settings watcher not loaded | Open `/hooks` menu or restart Claude Code |
| Hook fires on every Bash command | Wrong matcher in `settings.json` | Should be `Edit\|Write\|MultiEdit`, not `*` |
| Manual run fails: `awk: command not found` | POSIX awk missing | `brew install gawk` (macOS) or use WSL (Windows) |
| Stale comment masks a removal | Text-based grep matches comments too | Scrub *all* references when retiring an INV-* |

---

## See also

- `docs/spec.md` — the invariant definitions themselves.
- `docs/all_tests.md` § *Invariant Coverage Matrix* — which test exercises each INV-*.
- `docs/self-verified-pipeline.md` — broader picture of how spec, code,
  and tests connect.
- `.claude/CLAUDE.md` § *Invariants Convention* — the canonical policy
  statement that this document elaborates on.
