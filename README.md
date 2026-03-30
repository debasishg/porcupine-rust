# porcupine-rust

A Rust port of [porcupine](https://github.com/anishathalye/porcupine), a fast linearizability checker for testing the correctness of concurrent and distributed systems.

## What is Linearizability?

Linearizability is a correctness condition for concurrent systems. A history of concurrent operations is linearizable if the operations can be reordered — while respecting their real-time overlap — into a sequential execution that satisfies the system's sequential specification.

## Features

- Check linearizability of concurrent operation histories against a sequential model
- Support for both timestamped `Operation` and raw `Event` (call/return) history formats
- Optional timeout-based checking with a tri-state `CheckResult`
- P-compositional checking for partitionable models (e.g., key-value stores partitioned by key)
- Efficient DFS with backtracking, bitset-based state tracking, and caching

## Usage

```rust
use porcupine::{CheckResult, Model, Operation};

// Define a sequential model (e.g., a register)
// ...

// Check a history
let result = porcupine::check_operations(&model, &history);
assert_eq!(result, CheckResult::Ok);
```

## Status

Work in progress — Rust port of the [original Go implementation](https://github.com/anishathalye/porcupine).

## License

MIT — see [LICENSE](LICENSE).
