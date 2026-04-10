# Aeneas/Charon LLBC Formal Proofs for porcupine-rust

## Context

The project already has a strong verification pipeline:

- **Quint** (`tla/Porcupine.qnt`, `tla/NondeterministicModel.qnt`) — proves *the algorithm* satisfies safety invariants (`safetyInvariant`, `cacheSound`, `pCompositionality`, `powerSetSoundnessInv`)
- **proptest** — randomised evidence for all 9 `INV-*` identifiers
- **MBT** (`quint-mbt`) — Quint-generated traces replayed against the Rust checker

The gap: Quint operates on an *abstract model* of the algorithm. It cannot prove that the Rust *implementation* faithfully computes what the algorithm says. Specifically:

- Quint cannot prove `bitset.hash()` respects equality (INV-LIN-04 cache soundness depends on this at the code level)
- Quint cannot prove `deduplicate` is idempotent and sound (INV-ND-01)
- Quint cannot prove the DFS terminates or that soundness holds at the code level

Aeneas + Charon fills exactly this gap: Charon extracts LLBC from Rust MIR; Aeneas translates LLBC to **Lean 4**, enabling full functional-correctness proofs on the Rust code itself.

---

## Complementarity: Quint vs Aeneas

```
Quint (tla/)        →  proves the ALGORITHM is correct (abstract state machine)
Aeneas (verify/)    →  proves the RUST CODE computes what the algorithm says
debug_assert!       →  cheap runtime checks during testing
proptest            →  randomised evidence across all layers
```

Neither tool subsumes the other. Quint cannot reason about `Vec<u64>` bit operations or `deduplicate`'s loop body. Aeneas cannot prove protocol-level properties across concurrent state transitions. Together they form a full-stack verification argument.

---

## Architecture: `verify` Feature + Thin Toolchain Anchor

### The duplication problem and its solution

An earlier approach used a `verify/` crate that duplicated the hot modules
(`bitset_spec.rs`, `powerset_spec.rs`, `dfs_kernel.rs`). This meant Charon
proved properties about *copies* of the code, not the originals, creating a
silent drift risk.

The adopted approach eliminates all duplication via a **`verify` feature flag**
in the main crate. Charon runs directly on `porcupine` with `--features verify`:

```
src/bitset.rs           ──── verify feature ────▶  Bitset(Vec<u64>)         ──▶ Charon
src/model.rs            ──── unchanged ──────────▶  Model, PowerSetModel     ──▶ Charon
src/checker.rs          ──── verify feature ────▶  check_single (pure)      ──▶ Charon
```

There is **one source of truth**. A change to `src/bitset.rs` is automatically
reflected in the next Charon extraction run — no manual sync required.

### What the `verify` feature does

| Location | Production (default) | With `--features verify` |
|----------|---------------------|--------------------------|
| `src/bitset.rs` | `Bitset(SmallVec<[u64;4]>)` | `Bitset(Vec<u64>)` |
| `src/checker.rs` cache | `FxHashMap<u64, SmallVec<[CacheEntry;2]>>` | `HashMap<u64, Vec<CacheEntry>>` |
| `src/checker.rs` kill flag | `&AtomicBool` (via `KillSwitch` trait) | `&bool` (via `KillSwitch` trait) |
| `check_parallel`, `spawn_timer`, `check_operations`, `check_events` | compiled in | **compiled out** |
| `check_single`, `NodeArena`, `make_entries`, `deduplicate`, `PowerSetModel` | compiled in | compiled in (Charon targets) |

### `KillSwitch` trait

A small internal trait unifies the kill signal across production and verify modes,
keeping `check_single` generic without duplicating its body:

```rust
trait KillSwitch { fn is_killed(&self) -> bool; }
impl KillSwitch for bool        { fn is_killed(&self) -> bool { *self } }
impl KillSwitch for AtomicBool  { ... }     // non-verify only
impl KillSwitch for Arc<AtomicBool> { ... } // non-verify only
```

### Repository layout

```
Cargo.toml                    — [features] verify = []
src/
  bitset.rs                   — #[cfg(not/feature="verify")] SmallVec vs Vec<u64>
  checker.rs                  — KillSwitch trait; CacheMap type alias; gated concurrency
  model.rs                    — unchanged (already pure)
verify/
  Cargo.toml                  — workspace anchor; no source code
  rust-toolchain.toml         — nightly pin for Charon
  src/lib.rs                  — doc comment only (extraction workflow)
  lean/
    PorcupineVerify/
      lakefile.lean           — Mathlib4 + Aeneas Lean library
      BitsetSpec.lean         — Tier 1 proofs
      DeduplicateSpec.lean    — Tier 2a proofs
      PowerSetSpec.lean       — Tier 2b INV-ND-01 proof
      DfsKernel.lean          — Tier 3 termination + soundness sketch
```

---

## Scope Boundaries

### What Aeneas CAN handle (targets)

| Module | Notes |
|--------|-------|
| `src/bitset.rs` | `Bitset(Vec<u64>)` under verify feature; fully pure |
| `src/model.rs` — `deduplicate`, `PowerSetModel` | Already pure; unchanged |
| `src/checker.rs` — `check_single`, `NodeArena`, `make_entries` | Pure under verify feature; `KillSwitch = bool` |

### What Aeneas CANNOT handle (out of scope)

- `check_parallel` — rayon, `Arc<AtomicBool>`, `Mutex`, `Condvar` → compiled out under verify
- `spawn_timer` — background thread → compiled out under verify
- `FxHashMap` / `SmallVec` — swapped for std equivalents under verify
- `dyn Trait`, closures — not used in the verified subset

The concurrency layer remains covered by Quint's `parallelKillFlagInvariant` + proptest.

---

## Phase 1 — Toolchain Setup

1. Install Charon (requires nightly Rust matching `verify/rust-toolchain.toml`):
   ```bash
   # Check the nightly Charon needs:
   curl -s https://raw.githubusercontent.com/AeneasVerif/charon/main/rust-toolchain \
     | grep channel
   # Update verify/rust-toolchain.toml to match, then:
   cargo install charon-driver   # or build from AeneasVerif/charon source
   ```
2. Install Lean 4 + `elan`. The `lakefile.lean` references Mathlib4 and the Aeneas Lean library.

---

## Phase 2 — Tier 1: Bitset Proofs

**Source**: `src/bitset.rs` compiled with `--features verify` (`Bitset(Vec<u64>)`).

**Extraction**:
```bash
# From workspace root — Charon sees the real src/bitset.rs, not a copy:
charon --crate porcupine --features verify --dest verify/llbc/
aeneas verify/llbc/porcupine.llbc -backend lean -dest verify/lean/PorcupineVerify/
```

**Lean proofs** (`verify/lean/PorcupineVerify/BitsetSpec.lean`):

| Theorem | Statement | Supports |
|---------|-----------|---------|
| `set_idempotent` | `set (set b i) i = set b i` | INV-LIN-04 |
| `set_clear_roundtrip` | bit `i` initially 0 → `clear (set b i) i = b` | INV-LIN-04 |
| `equal_implies_hash_equal` | `b1 = b2 → b1.hash = b2.hash` | INV-LIN-04 |
| `get_after_set` | bit reads back true after set | INV-LIN-04 |

`equal_implies_hash_equal` closes the INV-LIN-04 code-level gap: equal bitsets always
land in the same cache bucket, so no valid (bitset, state) pair is ever pruned.

---

## Phase 3 — Tier 2: `deduplicate` + `PowerSetModel` Proofs

**Source**: `src/model.rs` (unchanged, already pure).

**Lean proofs** (`DeduplicateSpec.lean`):

| Theorem | Statement |
|---------|-----------|
| `deduplicate_no_duplicates` | `NoDup (deduplicate v)` |
| `deduplicate_subset` | `∀ x ∈ v, x ∈ deduplicate v` |
| `deduplicate_idempotent` | `deduplicate (deduplicate v) = deduplicate v` |

**Lean proofs** (`PowerSetSpec.lean`) — formal proof of **INV-ND-01**:

| Theorem | Statement |
|---------|-----------|
| `powerset_step_sound` | `M.step s i o ≠ []` for some `s ∈ state` → `PowerSetModel.step = Some _` |
| `powerset_step_complete` | `step = Some s'` → every element of `s'` reachable via `M.step` |
| `inv_nd_01` | if `M` accepts a sequence, `PowerSetModel(M)` accepts it too |

---

## Phase 4 — Tier 3: Pure DFS Kernel

**Source**: `src/checker.rs::check_single` compiled with `--features verify`.

Under the verify feature, `check_single<M, K: KillSwitch>` with `K = bool` is a
fully pure function (no atomics, no external crates). Charon extracts it directly.

### Termination (`DfsKernel.lean`)

Lexicographic measure `(active_count(arena), calls.len())` strictly decreases:
- Successful `model.step` → `lift` removes 2 nodes → `active_count` drops
- Cache hit / model rejection → cursor advances; exhaustion triggers backtrack → `calls.len()` drops

### Soundness Sketch (`DfsKernel.lean`)

```lean
theorem check_single_sound :
    check_single_pure model entries false = true →
    ∃ linearization, valid_linearization model history linearization := by
  sorry  -- three documented sub-lemmas
```

Sub-lemmas (documented in `DfsKernel.lean`):
1. `lift_preserves_validity` — lifting a call/return pair preserves residual history structure
2. `cache_prune_sound` — cached (bitset, state) means all branches from it were exhausted
3. `step_sequence_is_linearization` — accepted DFS steps form a valid sequential execution

---

## Invariant Coverage After This Work

| ID | Name | Quint | proptest | Aeneas |
|----|------|-------|----------|--------|
| INV-LIN-04 | Cache Soundness | ✓ abstract | ✓ | hash + equality proofs |
| INV-ND-01 | Power-Set Reduction Soundness | ✓ abstract | ✓ | Lean INV-ND-01 theorem |
| INV-LIN-01 | Soundness | ✓ abstract | ✓ | DFS soundness sketch |
| INV-LIN-02 | Completeness | ✓ abstract | ✓ | future (termination helps) |
| INV-HIST-01/02/03 | Well-Formed / Real-Time / Minimal Frontier | ✓ | ✓ | not targeted (structural) |

---

## Verification Workflow

```bash
# 1. Extract LLBC from the real source (no copy):
charon --crate porcupine --features verify --dest verify/llbc/

# 2. Translate to Lean 4:
aeneas verify/llbc/porcupine.llbc -backend lean -dest verify/lean/PorcupineVerify/

# 3. Check Lean proofs:
cd verify/lean/PorcupineVerify && lake build

# 4. Main crate unaffected — production build and tests:
cd ../../.. && cargo build
cargo clippy -- -D warnings
cargo test          # 128 tests pass

# 5. Quint still clean:
quint verify tla/Porcupine.qnt --invariant safetyInvariant
quint verify tla/NondeterministicModel.qnt --invariant safetyInvariant
```

**Success criterion**: `lake build` exits 0 with no `sorry` in Tier 1 and Tier 2 proofs.
Tier 3 soundness skeleton compiles with bounded `sorry` sub-lemmas, each documented
with its proof strategy.
