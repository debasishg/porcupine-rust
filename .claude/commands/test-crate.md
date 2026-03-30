# Run All Tests

Run the full test matrix for porcupine, covering all feature combinations.

## Feature Matrix

| Run | Command |
|-----|---------|
| Default (unit + property tests) | `cargo test` |
| Model-based tests (requires quint CLI) | `cargo test --features quint-mbt --test quint_mbt` |

## Steps

1. Run default tests:
   ```bash
   cargo test
   ```

2. Run MBT tests (only if `quint` CLI is available):
   ```bash
   quint --version && cargo test --features quint-mbt --test quint_mbt
   ```

3. Print a per-run status table. Show full output only on failure.

## Notes

- No `--release` requirement: porcupine has no unsafe/lock-free code at this stage.
- MBT tests require `quint` ≥ 0.31.0: `npm install -g @informalsystems/quint`.
- If `$ARGUMENTS` is a test name pattern, pass it as `cargo test <pattern>`.
