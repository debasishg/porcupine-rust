# Skills — porcupine-rust

Slash commands available in this project. Invoke with `/command-name` in Claude Code.

## Tier 1 — Orchestrators

| Skill | Command | Scope |
|-------|---------|-------|
| Full pre-merge verification | `/verify` | Workspace |

## Tier 2 — Verification (mandatory before merge)

| Skill | Command | Scope |
|-------|---------|-------|
| Quint model check + ITF trace replay | `/verify-spec` | `tla/Porcupine.qnt` |
| spec.md ↔ invariants.rs INV-* cross-check | `/verify-invariants` | All |

## Tier 3 — Testing

| Skill | Command | Scope |
|-------|---------|-------|
| All tests (unit + property + MBT) | `/test-crate` | porcupine |

## Tier 4 — Sync

| Skill | Command | Scope |
|-------|---------|-------|
| Diff INV-* IDs between spec.md and invariants.rs | `/spec-sync` | All |

---

## Composition

`/verify` composes Tier 2 skills:
```
/verify
├── /verify-spec
└── /verify-invariants
```

## Adding New Skills

1. Create `.claude/commands/<name>.md` with the executable prompt.
2. Add a row to the relevant tier table above.
3. Update `.claude/CLAUDE.md` skills quick-reference table.
