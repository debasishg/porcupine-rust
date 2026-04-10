//! Toolchain anchor for Charon/Aeneas extraction.
//!
//! This crate exists solely to:
//! 1. Carry `rust-toolchain.toml` — pins the nightly Charon requires.
//! 2. House the Lean 4 proof files under `lean/PorcupineVerify/`.
//!
//! **No Rust source code lives here.**  Charon is run directly on the main
//! `porcupine` crate with `--features verify`, which compiles out all
//! concurrency primitives and swaps external-crate types for std equivalents:
//!
//! ```text
//! # From the workspace root:
//! charon --crate porcupine --features verify --dest verify/llbc/
//! aeneas verify/llbc/porcupine.llbc -backend lean -dest verify/lean/PorcupineVerify/
//! cd verify/lean/PorcupineVerify && lake build
//! ```
//!
//! The `verify` feature gates in `src/`:
//! - `src/bitset.rs`  — `Bitset(Vec<u64>)` instead of `Bitset(SmallVec<[u64;4]>)`
//! - `src/checker.rs` — `CacheMap<S>` uses `HashMap` instead of `FxHashMap+SmallVec`;
//!                      `check_single` uses `KillSwitch` trait (`bool` impl);
//!                      `check_parallel`, `spawn_timer`, `check_operations`,
//!                      `check_events` compiled out entirely.
//! - `src/model.rs`   — unchanged (already pure).
