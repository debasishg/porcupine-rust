# porcupine-rust

A Rust port of [porcupine](https://github.com/anishathalye/porcupine), a fast linearizability checker for concurrent systems.

## Project Overview

This library checks whether a concurrent operation history is linearizable with respect to a sequential specification model. It is used to verify correctness of distributed systems and concurrent data structures.

## Build & Test

```bash
cargo build
cargo test
cargo clippy
```

## Code Style

- Follow standard Rust idioms and the Rust API Guidelines.
- Run `cargo clippy` and resolve all warnings before committing.
- Run `cargo fmt` to format code.
- Prefer `thiserror` for error types and `std` traits over custom ones where possible.
- Use `#[derive(Debug, Clone, PartialEq)]` on data types where applicable.

## Architecture

- `src/lib.rs` — public API and re-exports
- `src/model.rs` — `Model` trait and related types
- `src/checker.rs` — core linearizability checking algorithm (DFS + backtracking + caching)
- `src/bitset.rs` — compact bitset used to track linearized operations
- `src/types.rs` — `Operation`, `Event`, `CheckResult`, `LinearizationInfo`

## Key Concepts

- **Model**: A sequential specification — init state + step function + optional partition function.
- **Operation**: A completed concurrent operation with call/return timestamps.
- **Event**: A raw call or return event (alternative history representation).
- **CheckResult**: `Ok` (linearizable), `Illegal` (not linearizable), or `Unknown` (timeout).
- **Linearizability**: A history is linearizable if the concurrent operations can be reordered to a valid sequential execution consistent with real-time ordering.
